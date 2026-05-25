//! `frank memory` 子命令组: 通过 [`crate::sync_client::SyncClient`] 操作分布式记忆。
//!
//! # 命令清单
//!
//! ```text
//! frank memory add <content>      [--user] [--agent] [--session] [--metadata <json>]
//! frank memory add-raw <fact>     [--user] [--agent] [--session] [--metadata <json>]
//! frank memory search <query>     [--user] [--agent] [--limit] [--score-threshold]
//! frank memory list               [--user] [--agent] [--session] [--limit]
//! frank memory get <id>
//! frank memory delete <id>
//! frank memory healthz
//! ```
//!
//! 拆分:
//! - [`args`] — clap 派生的 Args / Subcommand 结构体
//! - [`handlers`] — 每个子命令的执行体, 调用 [`crate::sync_client::SyncClient`]

pub mod args;
pub mod handlers;
pub mod report;

use anyhow::Result;

pub use args::{
    AddArgs, AddRawArgs, Args, DeleteArgs, GetArgs, ListArgs, MemoryCommand, SearchArgs,
};

use crate::sync_client::SyncClient;

/// 执行 memory 子命令。
pub fn run(args: Args) -> Result<()> {
    tracing::debug!(?args, "memory invoked");

    let client = build_client(args.agent_url.as_deref())?;
    crate::log::ui::info(&format!("sync-agent: {}", client.base_url()));

    match args.command {
        MemoryCommand::Add(a) => handlers::run_add(&client, a),
        MemoryCommand::AddRaw(a) => handlers::run_add_raw(&client, a),
        MemoryCommand::Search(a) => handlers::run_search(&client, a),
        MemoryCommand::List(a) => handlers::run_list(&client, a),
        MemoryCommand::Get(a) => handlers::run_get(&client, a),
        MemoryCommand::Delete(a) => handlers::run_delete(&client, a),
        MemoryCommand::Healthz => handlers::run_healthz(&client),
    }
}

fn build_client(explicit: Option<&str>) -> Result<SyncClient> {
    match explicit {
        Some(url) => SyncClient::new(url),
        None => SyncClient::from_env_or_config(),
    }
}
