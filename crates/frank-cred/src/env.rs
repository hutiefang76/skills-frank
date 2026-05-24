//! Layer 1 fallback — env var 凭据。
//!
//! 读 `Provider::env_var_name()` 指向的环境变量, 非空则构造 [`Credential`]。
//! TokenKind 用 [`TokenKind::guess_from_token`] 启发式判断。

use chrono::Utc;
use secrecy::SecretString;

use crate::{Credential, CredentialSource, Provider, TokenKind};

/// 探 layer 1 env var。
///
/// 命中返回 `Some(Credential)`, 缺失或空值返回 `None`。
#[must_use]
pub fn lookup(provider: Provider) -> Option<Credential> {
    let name = provider.env_var_name();
    let raw = std::env::var(name).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        // 空值即缺失 (Claude Code 桌面 app 偶尔注 `ANTHROPIC_API_KEY=""`)
        return None;
    }

    let kind = TokenKind::guess_from_token(trimmed);
    Some(Credential {
        provider,
        kind,
        token: SecretString::from(trimmed.to_string()),
        expires_at: None,
        created_at: Utc::now(),
        source: CredentialSource::EnvVar {
            name: name.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    /// 测试时用同步 mutex 防止并行测改 env 互相打架。
    /// std::env 是进程全局, 任何 env-touching 测试都要序列化。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn lookup_missing_env_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        // 用一个绝不可能存在的 provider key 验 None (我们不能假设 ANTHROPIC_API_KEY 一定不存在)
        std::env::remove_var("ANTHROPIC_API_KEY");
        assert!(lookup(Provider::Claude).is_none());
    }

    #[test]
    fn lookup_empty_env_returns_none() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("OPENAI_API_KEY", "   ");
        assert!(lookup(Provider::Codex).is_none());
        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn lookup_present_env_returns_credential() {
        let _g = ENV_LOCK.lock().unwrap();
        let test_token = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
        std::env::set_var("ANTHROPIC_API_KEY", test_token);
        let cred = lookup(Provider::Claude).expect("应命中");
        assert_eq!(cred.token.expose_secret(), test_token);
        assert_eq!(cred.kind, TokenKind::LongLivedApiKey);
        assert!(matches!(cred.source, CredentialSource::EnvVar { .. }));
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}
