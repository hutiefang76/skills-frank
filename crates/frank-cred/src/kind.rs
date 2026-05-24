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
    /// # V3 实施修订 (2026-05-24, ADR-009 实施过程发现)
    ///
    /// V2 设计: OAuthSession 走 `PreserveOfficialFile` (不注 env, 让 child 自己读 file)。
    ///
    /// 实测发现: child CLI (claude --print) **同样** 因 macOS Keychain ACL 在
    /// 非 Anthropic 信任进程链中拿不到 token (它自己也是 keyring crate 调 Keychain)。
    /// 不注 env = child 也读不到 = 等于没修。
    ///
    /// V3: OAuthSession **也** InjectEnv (Anthropic 非交互模式 `claude --print` 见
    /// `ANTHROPIC_API_KEY` 就用, 不论 long-lived 还是 OAuth)。安全考量:
    /// - OAuth scope (`user:inference, user:mcp_servers` 等) 内部使用, 不算 scope 泄漏
    /// - frank 是合法 wrap, scope 同源
    /// - 失效风险: doctor 显示 `expires_at`, 失效前 7 天警告刷新
    #[must_use]
    pub fn export_strategy(self, provider: Provider) -> ExportStrategy {
        // V3: 三类都注 env, 差别仅在 doctor 显示警告 (OAuth 显示 expires_at).
        let _ = self; // 留 enum 区分供未来策略 (例 file-based child auth)
        ExportStrategy::InjectEnv(provider.env_var_name().to_string())
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
    fn oauth_session_also_injects_env_v3() {
        // V3 修订: OAuth 也注 env (V2 的 PreserveOfficialFile 实测不工作 — child 同 ACL 问题)
        let strat = TokenKind::OAuthSession.export_strategy(Provider::Claude);
        assert_eq!(
            strat,
            ExportStrategy::InjectEnv("ANTHROPIC_API_KEY".to_string())
        );
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
