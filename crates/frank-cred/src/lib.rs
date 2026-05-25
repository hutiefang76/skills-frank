//! `frank-cred` — 跨进程 CLI 凭据桥。
//!
//! # 解决的问题
//!
//! frank-cli 跨进程调 claude / codex / gemini / opencode 等第三方 CLI 时, 子进程
//! 可能因 macOS Keychain ACL / 缺 env 等原因拿不到凭据。本 crate 提供 **5 层 fallback**:
//!
//! 1. **env var** (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / ...) — 用户直配
//! 2. **frank store** (`~/.frank/credentials/<provider>.json`, mode 0600) — frank 自家
//! 3. **official file** (`~/.claude/.credentials.json` 等) — 各 CLI 自家 file
//! 4. **keyring** (macOS Keychain / Windows Credential Manager / Linux Secret Service) — best-effort
//! 5. **guidance** — 命中失败时返回 `Err`, 调用方给一行修复指令
//!
//! # 关键设计: TokenKind 区分
//!
//! 不是所有凭据都能安全注入 env var:
//! - [`TokenKind::LongLivedApiKey`] — 注 env (策略 D)
//! - [`TokenKind::OAuthSession`] — **不注 env**, 让 child 走自家 file
//! - [`TokenKind::ThirdPartyProxy`] — 注 env, doctor 警告
//!
//! # 使用
//!
//! ```no_run
//! use frank_cred::{Provider, resolve_and_inject};
//! use std::process::Command;
//!
//! let mut cmd = Command::new("claude");
//! cmd.arg("--print").arg("hello");
//!
//! match resolve_and_inject(&mut cmd, Provider::Claude) {
//!     Ok(report) => {
//!         eprintln!("✓ 凭据来源: {}", report.source);
//!         cmd.spawn().unwrap();
//!     }
//!     Err(e) => {
//!         eprintln!("✗ 凭据缺失: {e}");
//!         eprintln!("▶ 跑: frank login provider claude");
//!     }
//! }
//! ```

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod env;
pub mod kind;
pub mod official;
pub mod provider;
pub mod redact;
pub mod report;
pub mod store;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub mod keychain;

use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

pub use kind::{ExportStrategy, TokenKind};
pub use provider::Provider;
pub use redact::{redact_secrets, RedactWriter};
pub use report::{CallReport, CallSource};

/// 一条凭据 (含 provider / kind / 来源元数据)。
///
/// 注意: 不实现 `Serialize`/`Deserialize` (因为 `SecretString` 底层 `str` unsized)。
/// 磁盘持久化走 `store::StoreRecord` (token 用 `String`, 序列化后立刻 wrap 回 `SecretString`)。
#[derive(Debug)]
pub struct Credential {
    /// 所属 provider (claude / codex / ...)。
    pub provider: Provider,

    /// token 类型, 决定 export 策略。
    pub kind: TokenKind,

    /// 实际 token 字符串。`SecretString` 在 Drop 时 zero。
    pub token: SecretString,

    /// 过期时间 (若有)。
    pub expires_at: Option<DateTime<Utc>>,

    /// 创建时间。
    pub created_at: DateTime<Utc>,

    /// 凭据来源 (5 层 fallback 哪一层命中)。
    pub source: CredentialSource,
}

/// 凭据来源 (诊断用, doctor 显示, redact 显示前 6 字符)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialSource {
    /// 来自 env var (layer 1)。
    EnvVar {
        /// env var 名 (e.g. `ANTHROPIC_API_KEY`)。
        name: String,
    },
    /// 来自 frank store (layer 2)。
    FrankStore {
        /// 文件路径 (绝对路径)。
        path: std::path::PathBuf,
    },
    /// 来自 official file (layer 3)。
    Official {
        /// 文件路径 + provider 名。
        path: std::path::PathBuf,
    },
    /// 来自 keyring (layer 4)。
    Keyring {
        /// service name (e.g. `Claude Code-credentials`)。
        service: String,
    },
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvVar { name } => write!(f, "env:{name}"),
            Self::FrankStore { path } => write!(f, "frank:{}", path.display()),
            Self::Official { path } => write!(f, "official:{}", path.display()),
            Self::Keyring { service } => write!(f, "keyring:{service}"),
        }
    }
}

/// resolve_and_inject 结果报告。
#[derive(Debug)]
pub struct InjectReport {
    /// 凭据来源 (用于诊断/打印)。
    pub source: CredentialSource,

    /// token 是否注入了 env var (OAuthSession 走 file 不注 env)。
    pub injected_env: bool,

    /// 注入的 env var 名 (若 `injected_env=true`)。
    pub env_var: Option<String>,
}

/// `frank-cred` 的错误类型。
#[derive(Debug, thiserror::Error)]
pub enum CredError {
    /// 5 层 fallback 全 miss。
    #[error("provider {0} 凭据未命中任何 fallback 层 (env/frank-store/official/keyring)")]
    NotFound(Provider),

    /// I/O 错误 (读/写 frank store 或 official file)。
    #[error("io 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 序列化错误。
    #[error("serde 错误: {0}")]
    Serde(#[from] serde_json::Error),

    /// 其他。
    #[error("{0}")]
    Other(String),
}

/// resolve_and_inject 的 Result 别名。
pub type Result<T> = std::result::Result<T, CredError>;

/// 抽象掉具体 `Command` 类型 — frank-cli 用 [`std::process::Command`],
/// frank-orchestrator 用 [`tokio::process::Command`], 都能注入 env。
///
/// 调用方 crate 给 tokio Command 提供 `impl CommandEnv` (在自家定义)。
pub trait CommandEnv {
    /// 设置子进程 env var。
    fn set_env(&mut self, key: &str, value: &str);
    /// 删除子进程 env var (v0.11.2 OAuth 路径需要清空 inherited shell 的 *_API_KEY 防 401)。
    fn remove_env(&mut self, key: &str);
}

impl CommandEnv for std::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn remove_env(&mut self, key: &str) {
        std::process::Command::env_remove(self, key);
    }
}

/// `tokio-cmd` feature 启用时, 给 [`tokio::process::Command`] 实现 [`CommandEnv`]。
///
/// frank-orchestrator 启用此 feature (orchestrator/local.rs 用 tokio Command spawn child)。
#[cfg(feature = "tokio-cmd")]
impl CommandEnv for tokio::process::Command {
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
    fn remove_env(&mut self, key: &str) {
        tokio::process::Command::env_remove(self, key);
    }
}

/// 把 official credential file 一次性导入到 frank store。
///
/// 用于 `frank login provider <name>` 子命令: setup-token 跑完后,
/// frank 把 official file 内容复制到 `~/.frank/credentials/<provider>.json`
/// (mode 0600), 让后续 spawn 不再依赖官方 file 位置/格式/ACL。
///
/// # Errors
///
/// - official file 未找到 → [`CredError::NotFound`]
/// - I/O / 解析错误冒泡
pub fn import_official_to_store(provider: Provider) -> Result<std::path::PathBuf> {
    let cred = official::lookup(provider)?.ok_or(CredError::NotFound(provider))?;
    let token = cred.token.expose_secret().to_string();
    store::save(provider, &token, cred.kind)
}

/// 核心入口: 按 5 层 fallback 找凭据, 找到则按 `TokenKind` 决定要不要注 env, 返回报告。
///
/// `cmd` 是任意实现 [`CommandEnv`] 的类型 (std 或 tokio 的 Command)。
///
/// # Errors
///
/// 5 层全 miss 时返回 [`CredError::NotFound`], 调用方应给修复指引。
pub fn resolve_and_inject<C: CommandEnv>(cmd: &mut C, provider: Provider) -> Result<InjectReport> {
    // 1. env var
    if let Some(cred) = env::lookup(provider) {
        let source = cred.source.clone();
        return Ok(inject_per_kind(cmd, cred, source));
    }

    // 2. frank store
    if let Some(cred) = store::lookup(provider).ok().flatten() {
        let source = cred.source.clone();
        return Ok(inject_per_kind(cmd, cred, source));
    }

    // 3. official file
    if let Some(cred) = official::lookup(provider).ok().flatten() {
        let source = cred.source.clone();
        return Ok(inject_per_kind(cmd, cred, source));
    }

    // 4. keyring (best-effort)
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    if let Some(cred) = keychain::lookup(provider).ok().flatten() {
        let source = cred.source.clone();
        return Ok(inject_per_kind(cmd, cred, source));
    }

    Err(CredError::NotFound(provider))
}

/// 按 token kind 决定 export 策略。
fn inject_per_kind<C: CommandEnv>(
    cmd: &mut C,
    cred: Credential,
    source: CredentialSource,
) -> InjectReport {
    match cred.kind.export_strategy(cred.provider) {
        ExportStrategy::InjectEnv(env_name) => {
            cmd.set_env(&env_name, cred.token.expose_secret());
            tracing::debug!("frank-cred 注入 {env_name} (来源: {source})");
            InjectReport {
                source,
                injected_env: true,
                env_var: Some(env_name),
            }
        }
        ExportStrategy::PreserveOfficialFile => {
            // V4 (v0.11.2): OAuth session — 不注 env, child CLI 用自己 keychain.
            // 同时清掉继承 shell 里的空 *_API_KEY (防 401 陷阱).
            for env_key in &[
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
            ] {
                if std::env::var(env_key).is_ok_and(|v| v.trim().is_empty()) {
                    cmd.remove_env(env_key);
                }
            }
            tracing::debug!("frank-cred 检测到 OAuthSession, 不注 env (来源: {source})");
            InjectReport {
                source,
                injected_env: false,
                env_var: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_source_display() {
        let s = CredentialSource::EnvVar {
            name: "ANTHROPIC_API_KEY".to_string(),
        };
        assert_eq!(s.to_string(), "env:ANTHROPIC_API_KEY");
    }

    #[test]
    fn cred_error_not_found_display() {
        let e = CredError::NotFound(Provider::Claude);
        assert!(e.to_string().contains("claude"));
        assert!(e.to_string().contains("env/frank-store/official/keyring"));
    }
}
