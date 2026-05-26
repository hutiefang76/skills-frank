//! `frank tenant` — server tenant 管理 (v0.12.0).
//!
//! # 子命令
//!
//! - `frank tenant register`        — 把 ~/.frank/.token 重新注册到服务器 (幂等, 自愈用)
//! - `frank tenant status`          — 看 quota 用量 + 是否申请了删除
//! - `frank tenant delete`          — 申请删除我的数据 (14 天倒计时, 期间可取消)
//! - `frank tenant cancel-delete`   — 取消删除申请
//!
//! 服务端: `crates/frank-sync-agent/src/routes.rs` 4 个 `/tenant/*` 端点.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::sync_client::SyncClient;

/// `frank tenant` 顶层参数.
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令.
    #[command(subcommand)]
    pub command: TenantCommand,
}

/// `frank tenant` 子命令.
#[derive(Subcommand, Debug)]
pub enum TenantCommand {
    /// 重新注册 (服务器丢数据 / 切 server 时自愈).
    Register,
    /// 看当前 tenant 状态 (quota / 删除倒计时).
    Status,
    /// 申请删除 — 14 天后真删 (qdrant + sqlite). 倒计时内可 cancel-delete 撤销.
    Delete,
    /// 取消删除申请.
    CancelDelete,
    /// v0.13.0: 把这台机器 link 到已有 tenant (多机共享同一 namespace 用).
    Link,
    /// v0.13.0: 清掉本地 token + machine_id, 下次跑 provision 拿新 token.
    /// **谨慎** — 老 tenant 还在服务器, 你失去访问入口 (服务器看, 但 frank cli 不再认它).
    Reset,
    /// v0.14.0: 从服务端拉本 tenant 的"已装 skill 列表", 缺什么装什么 (跨机同步).
    /// 只装 frank-official / frank-recommended; 用户 --url 装的不会同步过来.
    Sync,
}

/// 派发器.
pub fn run(args: Args) -> Result<()> {
    match args.command {
        TenantCommand::Reset => reset_token(),
        cmd => {
            let client = SyncClient::from_env_or_config()?;
            match cmd {
                TenantCommand::Register => register(&client),
                TenantCommand::Status => status(&client),
                TenantCommand::Delete => request_deletion(&client),
                TenantCommand::CancelDelete => cancel_deletion(&client),
                TenantCommand::Link => link_machine(&client),
                TenantCommand::Sync => sync_skills(&client),
                TenantCommand::Reset => unreachable!(),
            }
        }
    }
}

/// v0.14: `frank tenant sync` — 拉服务端列表, 缺什么装什么.
fn sync_skills(client: &SyncClient) -> Result<()> {
    crate::log::ui::section("frank tenant sync — 跨机 skill 同步");
    let remote = client
        .tenant_skills_list()
        .context("拉服务端 tenant skills 列表失败")?;
    if remote.is_empty() {
        crate::log::ui::info(
            "服务端这个 tenant 还没记录 skill — 在 A 机装一些 (frank install nacos-ops),\
             B 机跑 `frank tenant sync` 就能拿到",
        );
        return Ok(());
    }
    crate::log::ui::info(&format!("服务端记录 {} 个 skill", remote.len()));

    let state = crate::state::State::load_default().context("load state.json")?;
    let installed: std::collections::HashSet<&str> =
        state.iter().map(|s| s.name.as_str()).collect();

    let (already, todo): (Vec<_>, Vec<_>) = remote
        .iter()
        .partition(|s| installed.contains(s.name.as_str()));

    if !already.is_empty() {
        crate::log::ui::info(&format!("本机已装: {} 个, 跳过", already.len()));
    }
    if todo.is_empty() {
        crate::log::ui::success("已跟服务端齐, 无需操作");
        return Ok(());
    }
    crate::log::ui::warn(&format!("本机缺 {} 个 — 开始装", todo.len()));
    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    for s in &todo {
        crate::log::ui::info(&format!("→ frank install {} ({})", s.name, s.visibility));
        let install_args = crate::cli::install::Args {
            name: Some(s.name.clone()),
            all: false,
            profile: None,
            skip_health_check: true,
            force: false,
            upgrade: false,
            url: None,
            r#ref: None,
        };
        match crate::cli::install::run(install_args) {
            Ok(()) => ok += 1,
            Err(e) => failed.push((s.name.clone(), format!("{e:#}"))),
        }
    }
    crate::log::ui::success(&format!("装好 {ok} 个 ({} 个失败)", failed.len()));
    for (name, err) in &failed {
        crate::log::ui::error(&format!("  `{name}`: {err}"));
    }
    Ok(())
}

/// v0.13.0: link 本机 fingerprint 到已有 tenant (X-Frank-Token 是已有 tenant 的).
fn link_machine(client: &SyncClient) -> Result<()> {
    let fp = crate::machine_id::collect_fingerprint();
    let resp = client.tenant_link_machine(&fp)?;
    let machine_code = resp
        .get("machine_code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let tid = resp
        .get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    crate::log::ui::success(&format!("已 link 本机到 tenant ({tid})"));
    crate::log::ui::info(&format!("machine_code = {machine_code}"));
    // 同时写 .machine_id 本地
    if let Some(home) = dirs::home_dir() {
        let _ = std::fs::write(home.join(".frank").join(".machine_id"), machine_code);
    }
    Ok(())
}

/// v0.13.0: 清本地 token + machine_id, 下次跑 frank 触发 provision 拿新 token.
/// 不调服务端 (老 tenant 留服务器, 用户想真删跑 frank tenant delete 走 14d 流程).
fn reset_token() -> Result<()> {
    let home = dirs::home_dir().context("locate home dir")?;
    let token_path = home.join(".frank").join(".token");
    let machine_path = home.join(".frank").join(".machine_id");
    let mut removed = 0;
    if token_path.exists() {
        std::fs::remove_file(&token_path)?;
        removed += 1;
    }
    if machine_path.exists() {
        std::fs::remove_file(&machine_path)?;
        removed += 1;
    }
    crate::log::ui::success(&format!("已清 {removed} 个本地文件 (~/.frank/.token, .machine_id)"));
    crate::log::ui::warn("注意: 老 tenant 仍在服务器, 数据没动. 真想删跑 `frank tenant delete`.");
    crate::log::ui::info("下次任何 frank 命令会触发 provision 拿新 token + 新 tenant namespace");
    Ok(())
}

fn register(client: &SyncClient) -> Result<()> {
    let resp = client.tenant_register()?;
    let already = resp
        .get("status")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| s == "already_registered");
    let tid = resp
        .get("tenant_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    if already {
        crate::log::ui::info(&format!("已注册 (tenant_id={tid}), 跳过"));
    } else {
        crate::log::ui::success(&format!("注册成功 (tenant_id={tid})"));
    }
    Ok(())
}

fn status(client: &SyncClient) -> Result<()> {
    match client.tenant_status() {
        Ok(s) => {
            let tid = s
                .get("tenant_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let created = s
                .get("created_at")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let records = s
                .get("records_count")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let deletion = s
                .get("deletion_scheduled_at")
                .and_then(serde_json::Value::as_i64);

            crate::log::ui::section("frank tenant 状态");
            println!("  tenant_id:     {tid}");
            println!("  created_at:    {created} (epoch)");
            println!("  records_used:  {records}");
            match deletion {
                Some(ts) => {
                    let now = chrono::Utc::now().timestamp();
                    let days_left = (ts - now) / 86400;
                    let human = chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
                        .map_or_else(|| "?".to_string(), |dt| dt.to_rfc3339());
                    crate::log::ui::warn(&format!(
                        "⏰ 已申请删除, {days_left} 天后真删 ({human}). 撤销: frank tenant cancel-delete"
                    ));
                }
                None => println!("  deletion:      未申请"),
            }
        }
        Err(e) => {
            crate::log::ui::error(&format!("查 status 失败: {e:#}"));
            crate::log::ui::info("可能未注册, 跑 `frank tenant register` 看看");
        }
    }
    Ok(())
}

fn request_deletion(client: &SyncClient) -> Result<()> {
    let resp = client.tenant_request_deletion()?;
    let scheduled = resp
        .get("deletion_scheduled_at")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let human = resp
        .get("real_delete_at_human")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let now = chrono::Utc::now().timestamp();
    let days = (scheduled - now) / 86400;
    crate::log::ui::warn(&format!(
        "已申请删除 — 你的数据将在 {days} 天后真删 ({human})"
    ));
    crate::log::ui::info("想撤回: frank tenant cancel-delete");
    crate::log::ui::info("立刻找人删: 邮件 hutiefang@gmail.com");
    Ok(())
}

fn cancel_deletion(client: &SyncClient) -> Result<()> {
    let _ = client.tenant_cancel_deletion()?;
    crate::log::ui::success("已取消删除申请 (数据继续保留)");
    Ok(())
}
