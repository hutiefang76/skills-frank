//! Layer 4 fallback — 跨平台 keyring/keychain。
//!
//! 通过 `keyring` crate v3 统一抽象:
//! - macOS: Keychain Services API (+ `security` CLI fallback 绕 ACL)
//! - Windows: Win32 Credential Manager (DPAPI 加密)
//! - Linux: Secret Service (libsecret, GNOME Keyring/KWallet)
//!
//! ACL 拒绝 / 服务不可用时返回 `Ok(None)` (best-effort, 不阻塞 fallback)。
//!
//! # macOS `security` CLI fallback (关键!)
//!
//! Anthropic Claude Code v2 把 OAuth token 只存 Keychain (service `Claude Code-credentials`),
//! 没有 file fallback。`keyring` crate 在非 Anthropic 信任进程里调 Keychain API 会**静默返回
//! itemNotFound** (ACL 拒绝, 但不报错)。
//!
//! 解决: 调系统 `security find-generic-password -s <svc> -a <acct> -w`。`security` CLI 在
//! macOS ACL 默认允许列表中, 能读到 keychain entry。结果是 JSON (Anthropic OAuth 格式) 或裸 token。

use chrono::Utc;
use keyring::Entry;
use secrecy::SecretString;
use serde::Deserialize;

use crate::{Credential, CredentialSource, Provider, Result, TokenKind};

/// 探 layer 4 keyring。
///
/// 优先用 keyring crate (跨平台), 失败时 (macOS only) fallback 到 `security` CLI。
///
/// best-effort: 服务不可用 / ACL 拒绝 / 条目不存在 → `Ok(None)`, 不报错。
///
/// # Errors
///
/// 仅 keyring crate 内部严重错误才冒泡。
pub fn lookup(provider: Provider) -> Result<Option<Credential>> {
    let service = provider.keyring_service();
    let account = whoami_or_default();

    // 1. 试 keyring crate (跨平台)
    if let Ok(entry) = Entry::new(service, &account) {
        if let Ok(pwd) = entry.get_password() {
            if !pwd.is_empty() {
                return Ok(Some(make_credential(provider, &pwd, service)));
            }
        } else {
            tracing::debug!("keyring crate miss ({service}/{account}), 试 macOS fallback");
        }
    }

    // 2. macOS fallback: `security` CLI (绕 keyring crate 的 ACL 限制)
    #[cfg(target_os = "macos")]
    if let Some(pwd) = macos_security_cli_fallback(service, &account) {
        return Ok(Some(make_credential(provider, &pwd, service)));
    }

    Ok(None)
}

fn make_credential(provider: Provider, raw: &str, service: &str) -> Credential {
    // 优先解析 Anthropic OAuth JSON 格式 (`{"claudeAiOauth":{"accessToken":"sk-ant-oat01-...","expiresAt":...}}`)
    let (token, kind, expires_at) = parse_anthropic_oauth_json(raw).unwrap_or_else(|| {
        // 不是 JSON 或解析失败 → 当裸 token, 启发式判断 kind
        (raw.to_string(), TokenKind::guess_from_token(raw), None)
    });

    Credential {
        provider,
        kind,
        token: SecretString::from(token),
        expires_at,
        created_at: Utc::now(),
        source: CredentialSource::Keyring {
            service: service.to_string(),
        },
    }
}

#[derive(Deserialize)]
struct AnthropicOauthWrapper {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<AnthropicOauth>,
}

#[derive(Deserialize)]
struct AnthropicOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>, // unix ms
}

/// 解析 Anthropic Keychain 存的 OAuth JSON, 返回 `(token, kind, expires_at)`。
///
/// 不是 JSON 或缺 accessToken 返回 None。
fn parse_anthropic_oauth_json(
    raw: &str,
) -> Option<(String, TokenKind, Option<chrono::DateTime<Utc>>)> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let parsed: AnthropicOauthWrapper = serde_json::from_str(trimmed).ok()?;
    let oauth = parsed.claude_ai_oauth?;
    let token = oauth.access_token?;
    if token.is_empty() {
        return None;
    }
    let expires = oauth
        .expires_at
        .and_then(chrono::DateTime::<Utc>::from_timestamp_millis);
    Some((token, TokenKind::OAuthSession, expires))
}

/// macOS fallback: spawn `security find-generic-password -s <svc> -a <acct> -w`。
///
/// `security` CLI 在 macOS ACL 默认允许, 能读 keychain entry, 输出到 stdout。
#[cfg(target_os = "macos")]
fn macos_security_cli_fallback(service: &str, account: &str) -> Option<String> {
    use std::process::Command;
    let out = Command::new("security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::debug!(
            "security CLI miss ({service}/{account}): exit {}",
            out.status
        );
        return None;
    }
    let pwd = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if pwd.is_empty() {
        None
    } else {
        tracing::debug!("security CLI 命中 ({service}/{account}, len={})", pwd.len());
        Some(pwd)
    }
}

/// 获取当前用户名 (跨平台)。失败 fallback 到 "default"。
fn whoami_or_default() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    /// 仅验"miss 时返回 None 不 panic"。
    /// 不能假设 CI 有 Secret Service / Keychain ACL, 因此不写 set/get round-trip。
    /// (round-trip 测交给开发者本机手动验。)
    #[test]
    fn lookup_does_not_panic_on_miss() {
        // 哪怕 ACL 拒绝 / 服务不可用, 也应返回 Ok(None) 不 panic / 不 Err
        let res = lookup(Provider::Claude);
        assert!(
            res.is_ok(),
            "keyring lookup 即便 miss 也应 Ok(None) — got: {res:?}"
        );
    }

    #[test]
    fn whoami_returns_nonempty() {
        let u = whoami_or_default();
        assert!(!u.is_empty(), "whoami fallback 应非空");
    }

    #[test]
    fn parse_anthropic_oauth_full_payload() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx","refreshToken":"sk-ant-ort01-yyy","expiresAt":1779654722032,"scopes":["user:inference"]}}"#;
        let (token, kind, expires) = parse_anthropic_oauth_json(raw).expect("应解析");
        assert!(token.starts_with("sk-ant-oat01-"));
        assert_eq!(kind, TokenKind::OAuthSession, "OAuth 必标 session");
        assert!(expires.is_some(), "expires_at 应被解析");
    }

    #[test]
    fn parse_anthropic_oauth_missing_returns_none() {
        assert!(parse_anthropic_oauth_json("not json").is_none());
        assert!(parse_anthropic_oauth_json("{}").is_none());
        assert!(parse_anthropic_oauth_json(r#"{"claudeAiOauth":{}}"#).is_none());
    }

    #[test]
    fn make_credential_with_oauth_json() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-zzzzzzzzzzzzzzzzzzzz","expiresAt":1779654722032}}"#;
        let cred = make_credential(Provider::Claude, raw, "Claude Code-credentials");
        assert!(cred.token.expose_secret().starts_with("sk-ant-oat01-"));
        assert_eq!(cred.kind, TokenKind::OAuthSession);
        assert!(cred.expires_at.is_some());
    }

    #[test]
    fn make_credential_with_raw_token() {
        let cred = make_credential(
            Provider::Codex,
            "sk-proj-rawkeyxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
            "Codex CLI",
        );
        // 裸 token, 不是 JSON, 走启发式 (长串 → LongLivedApiKey)
        assert_eq!(cred.kind, TokenKind::LongLivedApiKey);
        assert!(cred.token.expose_secret().starts_with("sk-proj-"));
    }
}
