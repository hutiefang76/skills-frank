//! Layer 3 fallback — 各 CLI 官方 credential file。
//!
//! 每个 provider 不同格式 / 路径, 按 `Provider::official_file_candidates()` 顺序探。

use std::fs;
use std::path::Path;

use chrono::Utc;
use secrecy::SecretString;
use serde::Deserialize;

use crate::{Credential, CredentialSource, Provider, Result, TokenKind};

/// 探 layer 3 official file。
///
/// 命中返回 `Ok(Some(Credential))`。所有候选都不存在或都解析失败返回 `Ok(None)`。
///
/// # Errors
///
/// IO 错误 (非 "file not found" 类) 会冒泡, 其它 (空文件 / 解析失败) 静默 fallthrough。
pub fn lookup(provider: Provider) -> Result<Option<Credential>> {
    for path in provider.official_file_candidates() {
        if !path.exists() {
            continue;
        }
        match parse_one(provider, &path) {
            Ok(Some(cred)) => return Ok(Some(cred)),
            Ok(None) => {}
            Err(e) => {
                tracing::debug!("official file 解析失败 {}: {e}", path.display());
            }
        }
    }
    Ok(None)
}

fn parse_one(provider: Provider, path: &Path) -> Result<Option<Credential>> {
    let raw = fs::read_to_string(path)?;
    match provider {
        Provider::Claude => parse_claude(&raw, path),
        Provider::Codex => parse_codex(&raw, path),
        Provider::Gemini => parse_gemini(&raw, path),
        Provider::Opencode => parse_opencode(&raw, path),
    }
}

// ============================================================================
// claude — ~/.claude/.credentials.json (JSON), 多种 shape, 历史/版本兼容
// ============================================================================

#[derive(Debug, Deserialize)]
struct ClaudeCreds {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOauth>,
    #[serde(rename = "primaryApiKey")]
    primary_api_key: Option<String>,
    token: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

fn parse_claude(raw: &str, path: &Path) -> Result<Option<Credential>> {
    let parsed: ClaudeCreds = serde_json::from_str(raw)?;

    // 优先: 长期 API key
    if let Some(key) = parsed.primary_api_key.or(parsed.api_key).or(parsed.token) {
        if !key.trim().is_empty() {
            return Ok(Some(make_credential(
                Provider::Claude,
                &key,
                TokenKind::LongLivedApiKey,
                path,
            )));
        }
    }

    // 次选: OAuth access token (不可注 env, 标 OAuthSession)
    if let Some(oauth) = parsed.claude_ai_oauth {
        if let Some(token) = oauth.access_token {
            if !token.trim().is_empty() {
                return Ok(Some(make_credential(
                    Provider::Claude,
                    &token,
                    TokenKind::OAuthSession,
                    path,
                )));
            }
        }
    }

    Ok(None)
}

// ============================================================================
// codex — ~/.codex/credentials.toml or auth.json, 多版本
// ============================================================================

#[derive(Debug, Deserialize)]
struct CodexAuthJson {
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    api_key: Option<String>,
    token: Option<String>,
}

fn parse_codex(raw: &str, path: &Path) -> Result<Option<Credential>> {
    // 试 TOML 再试 JSON
    if path.extension().and_then(|e| e.to_str()) == Some("toml") {
        let parsed: toml::Value =
            toml::from_str(raw).map_err(|e| crate::CredError::Other(format!("toml: {e}")))?;
        if let Some(key) = parsed
            .get("OPENAI_API_KEY")
            .or_else(|| parsed.get("api_key"))
            .or_else(|| parsed.get("token"))
            .and_then(|v| v.as_str())
        {
            return Ok(Some(make_credential(
                Provider::Codex,
                key,
                TokenKind::guess_from_token(key),
                path,
            )));
        }
        return Ok(None);
    }

    let parsed: CodexAuthJson = serde_json::from_str(raw)?;
    if let Some(key) = parsed.openai_api_key.or(parsed.api_key).or(parsed.token) {
        if !key.trim().is_empty() {
            return Ok(Some(make_credential(
                Provider::Codex,
                &key,
                TokenKind::guess_from_token(&key),
                path,
            )));
        }
    }
    Ok(None)
}

// ============================================================================
// gemini — ~/.gemini/credentials.json 或 ~/.config/gemini/credentials.json
// ============================================================================

#[derive(Debug, Deserialize)]
struct GeminiCreds {
    api_key: Option<String>,
    #[serde(rename = "GEMINI_API_KEY")]
    gemini_api_key: Option<String>,
    token: Option<String>,
}

fn parse_gemini(raw: &str, path: &Path) -> Result<Option<Credential>> {
    let parsed: GeminiCreds = serde_json::from_str(raw)?;
    if let Some(key) = parsed.api_key.or(parsed.gemini_api_key).or(parsed.token) {
        if !key.trim().is_empty() {
            return Ok(Some(make_credential(
                Provider::Gemini,
                &key,
                TokenKind::guess_from_token(&key),
                path,
            )));
        }
    }
    Ok(None)
}

// ============================================================================
// opencode — ~/.opencode/auth.json
// ============================================================================

#[derive(Debug, Deserialize)]
struct OpencodeAuth {
    api_key: Option<String>,
    token: Option<String>,
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
}

fn parse_opencode(raw: &str, path: &Path) -> Result<Option<Credential>> {
    let parsed: OpencodeAuth = serde_json::from_str(raw)?;
    if let Some(key) = parsed.api_key.or(parsed.openai_api_key).or(parsed.token) {
        if !key.trim().is_empty() {
            return Ok(Some(make_credential(
                Provider::Opencode,
                &key,
                TokenKind::guess_from_token(&key),
                path,
            )));
        }
    }
    Ok(None)
}

fn make_credential(provider: Provider, token: &str, kind: TokenKind, path: &Path) -> Credential {
    Credential {
        provider,
        kind,
        token: SecretString::from(token.trim().to_string()),
        expires_at: None,
        created_at: Utc::now(),
        source: CredentialSource::Official {
            path: path.to_path_buf(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_tmp(raw: &str, ext: &str) -> NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(&format!(".{ext}"))
            .tempfile()
            .unwrap();
        f.write_all(raw.as_bytes()).unwrap();
        f
    }

    #[test]
    fn parse_claude_api_key() {
        let f = write_tmp(
            r#"{"primaryApiKey": "sk-ant-test-xxxxxxxxxxxxxxxxxxxxxxxxxx"}"#,
            "json",
        );
        let cred = parse_claude(&std::fs::read_to_string(f.path()).unwrap(), f.path())
            .unwrap()
            .expect("应解析出 api_key");
        assert_eq!(cred.kind, TokenKind::LongLivedApiKey);
        assert_eq!(
            cred.token.expose_secret(),
            "sk-ant-test-xxxxxxxxxxxxxxxxxxxxxxxxxx"
        );
    }

    #[test]
    fn parse_claude_oauth_marked_session() {
        let f = write_tmp(
            r#"{"claudeAiOauth": {"accessToken": "oauth-session-token-xxxxxxxxxxxxxxxxxxxx"}}"#,
            "json",
        );
        let cred = parse_claude(&std::fs::read_to_string(f.path()).unwrap(), f.path())
            .unwrap()
            .expect("应解析 oauth");
        assert_eq!(cred.kind, TokenKind::OAuthSession, "OAuth 应标 session");
    }

    #[test]
    fn parse_codex_toml() {
        let f = write_tmp(
            r#"OPENAI_API_KEY = "sk-test-xxxxxxxxxxxxxxxxxxxxxxxxxx""#,
            "toml",
        );
        let cred = parse_codex(&std::fs::read_to_string(f.path()).unwrap(), f.path())
            .unwrap()
            .expect("应解析 toml");
        assert_eq!(
            cred.token.expose_secret(),
            "sk-test-xxxxxxxxxxxxxxxxxxxxxxxxxx"
        );
    }

    #[test]
    fn parse_gemini_json() {
        let f = write_tmp(
            r#"{"GEMINI_API_KEY": "AIza-test-xxxxxxxxxxxxxxxxxxxxxxxxxx"}"#,
            "json",
        );
        let cred = parse_gemini(&std::fs::read_to_string(f.path()).unwrap(), f.path())
            .unwrap()
            .expect("应解析 gemini");
        assert!(cred.token.expose_secret().starts_with("AIza"));
    }

    #[test]
    fn parse_empty_returns_none() {
        let f = write_tmp(r"{}", "json");
        assert!(
            parse_claude(&std::fs::read_to_string(f.path()).unwrap(), f.path())
                .unwrap()
                .is_none()
        );
    }
}
