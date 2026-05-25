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

use anyhow::Result;
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
}

/// 派发器.
pub fn run(args: Args) -> Result<()> {
    let client = SyncClient::from_env_or_config()?;
    match args.command {
        TenantCommand::Register => register(&client),
        TenantCommand::Status => status(&client),
        TenantCommand::Delete => request_deletion(&client),
        TenantCommand::CancelDelete => cancel_deletion(&client),
    }
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
