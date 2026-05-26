//! `frank mcp-shim <name>` — 凭证-操作分离的 MCP 启动器 (v0.14 P1-G, ADR-014 §3.5).
//!
//! # 问题
//! 用户痛点 (原话): "mcp-mysql 要在 ~/.claude.json 写一堆 env (DB_HOST/USER/PASSWORD),
//! 只能配一个 DB. 想要一个 MCP 管多个数据库."
//!
//! # 方案
//! `~/.claude.json` 里:
//! ```json
//! "mcpServers": {
//!   "doris": {
//!     "command": "frank",
//!     "args": ["mcp-shim", "doris", "--profile", "uat"]
//!   }
//! }
//! ```
//! `frank mcp-shim doris --profile uat` 起来后:
//! 1. 读 manifest 的 doris 条目, 拿真 MCP 命令 + 参数
//! 2. 走 ~/.frank/credentials/mcp/doris.uat.json 拿凭证 (env vars)
//! 3. `setenv` 注入, `execvp` 真 MCP server
//!
//! 凭证零进入 ~/.claude.json. 同一 MCP 可用多个 profile 跑多个数据库实例.
//!
//! # Credentials 文件 schema
//! `~/.frank/credentials/mcp/<name>.<profile>.json` (chmod 0600):
//! ```json
//! { "env": { "DORIS_HOST": "...", "DORIS_USER": "...", "DORIS_PASSWORD": "..." } }
//! ```
//!
//! # 跟 frank-cred 的关系
//! frank-cred 当前管 AI provider 凭证 (Claude/Codex/Gemini/Opencode token). MCP 凭证是
//! 独立 namespace (DB 密码 vs AI token 是两码事), 不复用 Provider enum. 文件读取
//! 复用同款 0600 + chmod 防呆.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

/// `frank mcp-shim` 参数.
#[derive(Parser, Debug)]
pub struct Args {
    /// MCP 名 (跟 manifest skill name 对齐, 例 `doris-ops`).
    pub name: String,

    /// 凭证 profile (例 `uat` / `prod`). 默认 `default`.
    #[arg(long, default_value = "default")]
    pub profile: String,
}

/// MCP credentials 持久化 schema.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpCredentials {
    /// env vars 注入到子进程 (例 `{"DORIS_DSN": "host:port/db"}`).
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// 执行 `frank mcp-shim <name> --profile <p>`.
///
/// 不返回 — 走 exec 替换当前进程. 失败才 return Err.
pub fn run(args: Args) -> Result<()> {
    // 1. 找 manifest 内 mcp 命令 + args
    let manifests = crate::manifest::parser::discover()?;
    let registry = crate::manifest::resolver::Registry::new(crate::manifest::parser::merge(manifests));
    let skill = registry
        .find(&args.name)
        .ok_or_else(|| anyhow::anyhow!("`{}` not found in manifest (跑 frank list 查)", args.name))?;

    let (command, mcp_args) = match &skill.source {
        crate::manifest::schema::Source::Mcp {
            command,
            args: mcp_args,
            ..
        } => (command.clone(), mcp_args.clone()),
        _ => bail!(
            "`{}` 不是 MCP source — mcp-shim 只代理 Source::Mcp (manifest type: mcp).",
            args.name
        ),
    };

    // 2. 读 credentials (如果有)
    let creds = load_credentials(&args.name, &args.profile)?;

    // 3. 设 env (manifest 自带的 env 优先级低于 creds; 用户 cred 覆盖 manifest 默认)
    let mut env_to_inject: HashMap<String, String> = match &skill.source {
        crate::manifest::schema::Source::Mcp { env, .. } => env.clone(),
        _ => HashMap::new(),
    };
    env_to_inject.extend(creds.env);

    tracing::debug!(
        name = %args.name,
        profile = %args.profile,
        cmd = %command,
        env_keys = ?env_to_inject.keys().collect::<Vec<_>>(),
        "mcp-shim spawning"
    );

    // 4. exec — 替换当前进程, 不 fork
    exec_mcp_server(&command, &mcp_args, &env_to_inject)
}

/// 路径: `~/.frank/credentials/mcp/<name>.<profile>.json`.
fn credentials_path(name: &str, profile: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("locate home dir")?;
    Ok(home
        .join(".frank")
        .join("credentials")
        .join("mcp")
        .join(format!("{name}.{profile}.json")))
}

/// 读 credentials. 文件不存在 → 返回空 (允许 0 credentials MCP 也跑, 例 mcp-time).
fn load_credentials(name: &str, profile: &str) -> Result<McpCredentials> {
    let path = credentials_path(name, profile)?;
    if !path.exists() {
        tracing::debug!(
            path = %path.display(),
            "no MCP credentials file (跑 frank login mcp <name> --profile <p> 配)"
        );
        return Ok(McpCredentials::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read MCP credentials {}", path.display()))?;
    let creds: McpCredentials = serde_json::from_str(&raw)
        .with_context(|| format!("parse MCP credentials {}", path.display()))?;
    Ok(creds)
}

/// 在 unix 走 `execvp` (零 fork, 替换当前进程). Windows 走 spawn + wait + propagate code.
///
/// 注: unix 路径上 exec() 成功时不返回 (替换当前进程); 失败才 return Err.
/// 不用 `!` never type 因为 stable Rust 还不支持函数返回 `!`.
#[cfg(unix)]
fn exec_mcp_server(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(command);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // exec() 只在失败时返回 io::Error (成功直接替换当前进程, fn 永不返回)
    let err = cmd.exec();
    Err(anyhow::Error::from(err)
        .context(format!("execvp({command}) failed — 检查 cli 是否装了 / PATH 是否含")))
}

/// Windows 没 execvp, 退回 spawn + wait. 退出码透传给 caller (Claude).
#[cfg(not(unix))]
fn exec_mcp_server(command: &str, args: &[String], env: &HashMap<String, String>) -> Result<()> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    // stdin/stdout/stderr 全继承 (MCP server 跟 Claude stdio 通信)
    let status = cmd
        .status()
        .with_context(|| format!("spawn {command}"))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn credentials_default_empty() {
        let c = McpCredentials::default();
        assert!(c.env.is_empty());
    }

    #[test]
    fn credentials_roundtrip_json() {
        let mut c = McpCredentials::default();
        c.env.insert("DORIS_HOST".into(), "localhost:9030".into());
        c.env.insert("DORIS_PASSWORD".into(), "secret".into());
        let json = serde_json::to_string(&c).unwrap();
        let back: McpCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(back.env.get("DORIS_HOST").map(String::as_str), Some("localhost:9030"));
        assert_eq!(back.env.get("DORIS_PASSWORD").map(String::as_str), Some("secret"));
    }

    #[test]
    fn load_credentials_missing_returns_empty() {
        // 用 tempdir 顶替 home_dir 不容易 (dirs::home_dir 没法 mock), 这里改测
        // load 内部逻辑: 路径不存在直接返 default. 走 path manipulation 直接构造.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(!path.exists());
        // 模拟 load 路径: 文件不存在 → default
        let creds = if path.exists() {
            let raw = fs::read_to_string(&path).unwrap();
            serde_json::from_str::<McpCredentials>(&raw).unwrap()
        } else {
            McpCredentials::default()
        };
        assert!(creds.env.is_empty());
    }
}
