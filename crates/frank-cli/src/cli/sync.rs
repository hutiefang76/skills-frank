//! `frank sync` 子命令 — 跨设备 skills 同步 (用户需求 2.3)。
//!
//! 典型场景: 用户 mac 装了一堆 skills (含 MCP), 想 windows 上一键复制. 流程:
//!
//! ```text
//!  Mac:     frank sync push       → state.json 推到 sync-agent
//!  Win:     frank sync pull mac   → 拉 mac 的 state, install 本机缺的 skill
//! ```
//!
//! v0.4 简单模型: 服务端按 device_id KV 存, 重启丢. v0.5 改 SQLite 持久.

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;

use crate::state::State;
use crate::sync_client::SyncClient;

/// `frank sync` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: SyncCommand,
}

/// `frank sync` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum SyncCommand {
    /// 把本机 state.json 推到 sync-agent (默认 device_id = hostname)。
    Push(PushArgs),
    /// 从某 device 拉 state, 列出对方装了但本机没装的 skills。
    Pull(PullArgs),
    /// 列出所有 已 push 过的 device。
    Devices,
}

/// `frank sync push` 参数。
#[derive(Parser, Debug)]
pub struct PushArgs {
    /// 设备 ID (默认 hostname)。
    #[arg(long)]
    pub device_id: Option<String>,
}

/// `frank sync pull` 参数。
#[derive(Parser, Debug)]
pub struct PullArgs {
    /// 从哪台设备拉 state (device_id, 如 `hutiefang-mac`)。
    pub from: String,

    /// 仅列对方有 / 本机无的 skill (默认行为)。
    #[arg(long)]
    pub diff_only: bool,
}

/// 执行 sync 命令。
pub fn run(args: Args) -> Result<()> {
    match args.command {
        SyncCommand::Push(p) => run_push(p),
        SyncCommand::Pull(p) => run_pull(p),
        SyncCommand::Devices => run_devices(),
    }
}

fn run_push(args: PushArgs) -> Result<()> {
    let device_id = args
        .device_id
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().to_string());
    let state = State::load_default()?;
    let json = serde_json::to_value(StateJsonView {
        schema_version: 1,
        profile: "personal".to_string(),
        skills: state.iter().map(|s| (s.name.clone(), s.clone())).collect(),
    })?;

    let client = SyncClient::from_env_or_config()?;
    let url = format!("{}/sync/skills/push", client.base_url());
    let http = reqwest::blocking::Client::new();
    let resp = http
        .post(&url)
        .json(&serde_json::json!({
            "device_id": device_id,
            "state": json,
        }))
        .header("X-Frank-Token", read_token().unwrap_or_default())
        .send()
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let body = resp.text().context("read body")?;
    if !status.is_success() {
        anyhow::bail!("sync push failed ({status}): {body}");
    }
    crate::log::ui::success(&format!(
        "推送 device `{device_id}` ({} skills) → {}",
        state.len(),
        client.base_url()
    ));
    println!("{body}");
    Ok(())
}

fn run_pull(args: PullArgs) -> Result<()> {
    let client = SyncClient::from_env_or_config()?;
    let url = format!(
        "{}/sync/skills/pull?device_id={}",
        client.base_url(),
        urlencoding(&args.from)
    );
    let http = reqwest::blocking::Client::new();
    let resp = http
        .get(&url)
        .header("X-Frank-Token", read_token().unwrap_or_default())
        .send()
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().context("read body")?;
    if !status.is_success() {
        anyhow::bail!("sync pull failed ({status}): {body}");
    }

    let remote: StateJsonView = serde_json::from_str(&body).context("decode remote state.json")?;
    let local = State::load_default()?;
    let local_names: std::collections::HashSet<&str> =
        local.iter().map(|s| s.name.as_str()).collect();

    let missing: Vec<&String> = remote
        .skills
        .keys()
        .filter(|k| !local_names.contains(k.as_str()))
        .collect();

    if missing.is_empty() {
        crate::log::ui::success(&format!(
            "本机与 device `{}` 已同步 ({} skills 一致)",
            args.from,
            remote.skills.len()
        ));
        return Ok(());
    }

    crate::log::ui::section(&format!(
        "Device `{}` 装了 {} 个 skill, 本机缺 {} 个:",
        args.from,
        remote.skills.len(),
        missing.len()
    ));
    for name in &missing {
        println!("  - {name}");
    }
    crate::log::ui::info("运行 `frank install <name>` 装上 (或 v0.5 加 `frank sync apply` 批量)");
    Ok(())
}

fn run_devices() -> Result<()> {
    let client = SyncClient::from_env_or_config()?;
    let url = format!("{}/sync/skills/devices", client.base_url());
    let http = reqwest::blocking::Client::new();
    let resp = http
        .get(&url)
        .header("X-Frank-Token", read_token().unwrap_or_default())
        .send()
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().context("read body")?;
    if !status.is_success() {
        anyhow::bail!("sync devices failed ({status}): {body}");
    }
    let resp: DevicesResp = serde_json::from_str(&body).context("decode")?;
    crate::log::ui::section(&format!("已 push 设备 ({} 台):", resp.devices.len()));
    for d in &resp.devices {
        println!("  - {}: {} skills", d.device_id, d.skills_count);
    }
    Ok(())
}

#[derive(serde::Serialize, Deserialize)]
struct StateJsonView {
    schema_version: u32,
    profile: String,
    skills: std::collections::BTreeMap<String, crate::state::SkillState>,
}

#[derive(Deserialize)]
struct DevicesResp {
    devices: Vec<DeviceInfo>,
}

#[derive(Deserialize)]
struct DeviceInfo {
    device_id: String,
    skills_count: usize,
}

fn read_token() -> Option<String> {
    if let Ok(t) = std::env::var("FRANK_API_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    let path = dirs::home_dir()?.join(".frank").join(".token");
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn urlencoding(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// 重新导出 anyhow::anyhow 在 doc 里以防未来某天 anyhow 改名 — clippy 满意。
#[allow(dead_code)]
fn _doc_anchor() {
    let _ = anyhow!("doc");
}
