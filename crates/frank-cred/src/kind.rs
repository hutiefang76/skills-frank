//! `TokenKind` 体系 — 决定凭据的 export 策略。
//!
//! V2 关键改进 (ADR-009 修 codex Plan Review dim_3): 不是所有 token 都能安全注 env。
//! - `LongLivedApiKey` (e.g. `sk-ant-...`) — 注 env, child CLI 优先读 env 绕 Keychain
//! - `OAuthSession` (短期, scope-bound) — **不注 env**, child 自己走 file path
//! - `ThirdPartyProxy` (中转站 key) — 注 env, doctor 警告

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

/// token 类型, 决定 export 策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    /// 长期 API key (e.g. `sk-ant-...`, `sk-proj-...`)。
    ///
    /// 可安全作为 env var (`ANTHROPIC_API_KEY` 等) 注入 child 进程。
    LongLivedApiKey,

    /// OAuth session token (短期 / scope-bound)。
    ///
    /// **不可** 直接注 env, 原因:
    /// - 跨 scope 泄漏 (OAuth scope ≠ API key scope)
    /// - session 短期失效后 child CLI 不会触发 OAuth 重新登录
    /// - 不同 provider 的 OAuth token 格式不同 (Anthropic 与 OpenAI 不互通)
    ///
    /// 这种 token 走 [`ExportStrategy::PreserveOfficialFile`] — 不注 env, 假定 child
    /// CLI 自己能找到自家 official file (frank 只保证 env 未污染, 不主动注入)。
    OAuthSession,

    /// 第三方代理 / 中转站 key (例如自部署 cli-proxy-api 的 key)。
    ///
    /// 注 env (兼容用法), 但 `frank doctor` 显眼黄色警告:
    /// "该 key 不是 provider 官方 endpoint, 数据流经第三方"。
    ThirdPartyProxy,
}

/// export 策略 — `resolve_and_inject` 根据 [`TokenKind`] 选其一。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportStrategy {
    /// 注入指定 env var 名 (e.g. `ANTHROPIC_API_KEY`)。
    InjectEnv(String),

    /// 不注 env, 让 child CLI 自己走 official file path。
    ///
    /// frank 这边不动 env, 假定 child 会自己读 `~/.claude/.credentials.json` 等。
    PreserveOfficialFile,
}

impl TokenKind {
    /// 推导该 token 对应 provider 的 export 策略。
    ///
    /// # V4 实施修订 (2026-05-25, v0.11.2 实测用户报错后)
    ///
    /// V2 设计: OAuthSession 走 `PreserveOfficialFile` (不注 env, 让 child 自己读 file)。
    /// V3 修订: 全部 InjectEnv (因为同进程链 Keychain ACL 拿不到, 注 env 才能给 child)。
    /// **V3 错了**: claude --print 见到 `ANTHROPIC_API_KEY` 会把它当 *long-lived API key*
    /// 调 api.anthropic.com, 但 OAuth session token 不是 API key 格式, 直接 401.
    ///
    /// 实际现象 (用户实测):
    /// ```text
    /// $ frank ai ask --to claude --model sonnet "你好"
    /// [frank-cred] OK ANTHROPIC_API_KEY (source: keyring:Claude Code-credentials)
    /// ERROR `claude` exit 1
    /// ```
    /// 而 `claude --print "你好"` 直接跑没问题 (走自家 keychain ACL OK).
    ///
    /// V4 修复: OAuthSession 回 `PreserveOfficialFile` — frank 不注 env,
    /// child claude 用自己 keychain 走自己 OAuth, **不被 frank 干扰**。
    /// `LongLivedApiKey` (`sk-ant-` / `sk-` 开头) 仍 InjectEnv (那才是 child 期望的格式).
    /// `ThirdPartyProxy` 仍 InjectEnv (代理 key 就是 API key 形态).
    #[must_use]
    pub fn export_strategy(self, provider: Provider) -> ExportStrategy {
        match self {
            Self::LongLivedApiKey | Self::ThirdPartyProxy => {
                ExportStrategy::InjectEnv(provider.env_var_name().to_string())
            }
            Self::OAuthSession => {
                // 不注 env, 让 child CLI 用自己的 keychain OAuth session.
                // 注: 同时 inject_per_kind 会调 strip_empty_api_keys 清掉继承的空值, 防 trap.
                ExportStrategy::PreserveOfficialFile
            }
        }
    }

    /// 从原始 token 字符串启发式判断 kind (用于 import / migration)。
    ///
    /// 启发式 (粗略, 失败回退 LongLivedApiKey):
    /// - 含 `oauth` / `session` / 长度 < 30 → `OAuthSession`
    /// - 否则 → `LongLivedApiKey` (大部分 api key)
    #[must_use]
    pub fn guess_from_token(token: &str) -> Self {
        let lower = token.to_lowercase();
        if lower.contains("oauth") || lower.contains("session") || token.len() < 30 {
            Self::OAuthSession
        } else {
            Self::LongLivedApiKey
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lived_api_key_injects_env() {
        let strat = TokenKind::LongLivedApiKey.export_strategy(Provider::Claude);
        assert_eq!(
            strat,
            ExportStrategy::InjectEnv("ANTHROPIC_API_KEY".to_string())
        );
    }

    #[test]
    fn oauth_session_preserves_official_file_v4() {
        // V4 修复 (v0.11.2): OAuth 不注 env, claude 用自己 keychain.
        // V3 的注 env 让 claude 把 OAuth session 当 API key 调 → 401.
        let strat = TokenKind::OAuthSession.export_strategy(Provider::Claude);
        assert_eq!(strat, ExportStrategy::PreserveOfficialFile);
    }

    #[test]
    fn third_party_proxy_injects_env() {
        let strat = TokenKind::ThirdPartyProxy.export_strategy(Provider::Codex);
        assert_eq!(
            strat,
            ExportStrategy::InjectEnv("OPENAI_API_KEY".to_string())
        );
    }

    #[test]
    fn guess_short_token_is_oauth() {
        assert_eq!(
            TokenKind::guess_from_token("oauth-abc"),
            TokenKind::OAuthSession
        );
        assert_eq!(
            TokenKind::guess_from_token("short"),
            TokenKind::OAuthSession
        );
    }

    #[test]
    fn guess_long_api_key() {
        let long = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
        assert_eq!(
            TokenKind::guess_from_token(long),
            TokenKind::LongLivedApiKey
        );
    }
}
