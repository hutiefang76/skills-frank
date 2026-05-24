//! Layer 2 fallback — frank 自家 credential store。
//!
//! 路径: `~/.frank/credentials/<provider>.json`
//! 权限: Unix `0600` (文件) + `0700` (目录), Windows 设当前用户独占。
//!
//! 这是 `frank login provider <name>` 写入的目标, 也是 5 层 fallback 的第 2 层。

use std::fs;
use std::path::{Path, PathBuf};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::{CredError, Credential, CredentialSource, Provider, Result, TokenKind};

/// frank store 目录: `~/.frank/credentials/`。
///
/// # Errors
///
/// `dirs::home_dir()` 失败 (极少, 通常意味着 HOME 未设)。
pub fn store_dir() -> Result<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| CredError::Other("找不到 home 目录 (HOME 未设?)".to_string()))?;
    Ok(home.join(".frank").join("credentials"))
}

/// 指定 provider 的 store 文件路径。
///
/// # Errors
///
/// 同 [`store_dir`]。
pub fn store_path(provider: Provider) -> Result<PathBuf> {
    Ok(store_dir()?.join(format!("{provider}.json")))
}

/// store JSON 的磁盘 schema (区别于内存 [`Credential`], 因 `SecretString` 需要序列化为字符串)。
#[derive(Debug, Serialize, Deserialize)]
struct StoreRecord {
    provider: Provider,
    kind: TokenKind,
    token: String,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    bootstrap_source: BootstrapSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BootstrapSource {
    /// 通过 `frank login provider <name>` 自动 wrap official setup 命令。
    SetupToken { wrapped_command: String },
    /// 用户手动 `frank login provider <name> --token <key>` 注入。
    ManualToken,
    /// 从 env var 一次性 import 进 store。
    ImportedFromEnv { env_var: String },
}

/// 探 layer 2 frank store。
///
/// 命中返回 `Ok(Some(Credential))`。文件不存在返回 `Ok(None)`。解析失败返回 `Err`。
///
/// # Errors
///
/// 路径/IO/JSON 错误。
pub fn lookup(provider: Provider) -> Result<Option<Credential>> {
    let path = store_path(provider)?;
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(&path)?;
    let rec: StoreRecord = serde_json::from_str(&raw)?;

    Ok(Some(Credential {
        provider: rec.provider,
        kind: rec.kind,
        token: SecretString::from(rec.token),
        expires_at: rec.expires_at,
        created_at: rec.created_at,
        source: CredentialSource::FrankStore { path },
    }))
}

/// 写 layer 2 frank store, 自动设安全权限。
///
/// # Errors
///
/// 路径/IO/JSON/权限错误。
pub fn save(provider: Provider, token: &str, kind: TokenKind) -> Result<PathBuf> {
    let dir = store_dir()?;
    fs::create_dir_all(&dir)?;
    secure_dir(&dir)?;

    let rec = StoreRecord {
        provider,
        kind,
        token: token.to_string(),
        expires_at: None,
        created_at: chrono::Utc::now(),
        bootstrap_source: BootstrapSource::SetupToken {
            wrapped_command: format!("frank login provider {provider}"),
        },
    };

    let path = store_path(provider)?;
    let json = serde_json::to_string_pretty(&rec)?;
    fs::write(&path, json)?;
    secure_file(&path)?;
    Ok(path)
}

/// 删 layer 2 frank store 中的 provider 凭据。
///
/// # Errors
///
/// 路径/IO 错误。
pub fn remove(provider: Provider) -> Result<bool> {
    let path = store_path(provider)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path)?;
    Ok(true)
}

/// 列出 store 中所有已存的 provider。
///
/// # Errors
///
/// 目录读取错误。目录不存在返回空 Vec。
pub fn list() -> Result<Vec<Provider>> {
    let dir = store_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stem) = name.strip_suffix(".json") {
            if let Ok(p) = Provider::parse_name(stem) {
                out.push(p);
            }
        }
    }
    out.sort_by_key(std::string::ToString::to_string);
    Ok(out)
}

#[cfg(unix)]
fn secure_dir(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o700);
    fs::set_permissions(p, perms)?;
    Ok(())
}

#[cfg(unix)]
fn secure_file(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(p, perms)?;
    Ok(())
}

#[cfg(windows)]
fn secure_dir(_p: &Path) -> Result<()> {
    // Windows ACL: 默认继承用户 profile ACL (仅当前用户), 不需额外动作。
    // 严格场景留 V0.10.5 加 Win32 SetNamedSecurityInfo 移除 Users 组。
    tracing::debug!("Windows: secure_dir 默认继承用户 profile ACL");
    Ok(())
}

#[cfg(windows)]
fn secure_file(_p: &Path) -> Result<()> {
    tracing::debug!("Windows: secure_file 默认继承用户 profile ACL");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;
    use tempfile::TempDir;

    /// 测试用: 把 HOME 临时指向 tempdir, 避免污染真实 ~/.frank。
    fn with_temp_home<F: FnOnce()>(f: F) {
        static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = HOME_LOCK.lock().unwrap();

        let tmp = TempDir::new().unwrap();
        let old = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        f();
        if let Some(h) = old {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn save_then_lookup_roundtrip() {
        with_temp_home(|| {
            let saved_path = save(
                Provider::Claude,
                "sk-ant-api03-test-1234567890abcdefghij",
                TokenKind::LongLivedApiKey,
            )
            .unwrap();
            assert!(saved_path.exists());

            let cred = lookup(Provider::Claude).unwrap().expect("应命中");
            assert_eq!(
                cred.token.expose_secret(),
                "sk-ant-api03-test-1234567890abcdefghij"
            );
            assert_eq!(cred.kind, TokenKind::LongLivedApiKey);
            assert!(matches!(cred.source, CredentialSource::FrankStore { .. }));
        });
    }

    #[test]
    fn lookup_missing_returns_none() {
        with_temp_home(|| {
            assert!(lookup(Provider::Gemini).unwrap().is_none());
        });
    }

    #[test]
    fn remove_present_returns_true_missing_false() {
        with_temp_home(|| {
            save(
                Provider::Codex,
                "sk-test-xxxxxxxxxxxxxxxxxxxx",
                TokenKind::LongLivedApiKey,
            )
            .unwrap();
            assert!(remove(Provider::Codex).unwrap());
            assert!(!remove(Provider::Codex).unwrap());
        });
    }

    #[test]
    fn list_returns_saved_providers() {
        with_temp_home(|| {
            save(
                Provider::Claude,
                "tok1234567890abcdefghij1234567890",
                TokenKind::LongLivedApiKey,
            )
            .unwrap();
            save(
                Provider::Gemini,
                "tok2345678901bcdefghijk1234567890",
                TokenKind::LongLivedApiKey,
            )
            .unwrap();
            let mut got = list().unwrap();
            got.sort_by_key(std::string::ToString::to_string);
            assert_eq!(got, vec![Provider::Claude, Provider::Gemini]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home(|| {
            let p = save(
                Provider::Opencode,
                "tok-xxxxxxxxxxxxxxxxxxxxxxxxxx",
                TokenKind::LongLivedApiKey,
            )
            .unwrap();
            let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
        });
    }
}
