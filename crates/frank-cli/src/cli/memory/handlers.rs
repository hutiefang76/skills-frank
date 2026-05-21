//! `frank memory <sub>` 各子命令的运行体。
//!
//! `mod.rs` 仅做 dispatch, 具体业务在这里, 便于单文件 < 300 行。

use anyhow::{anyhow, Context, Result};
use frank_memory::{MemoryId, Scope};

use super::args::{AddArgs, AddRawArgs, DeleteArgs, GetArgs, ListArgs, SearchArgs};
use crate::sync_client::SyncClient;

/// 把 (user, agent, session) 三个可选字符串收进 `Scope`。
pub fn scope_of(user: Option<String>, agent: Option<String>, session: Option<String>) -> Scope {
    Scope {
        user_id: user,
        agent_id: agent,
        session_id: session,
    }
}

/// 解析 `--metadata` JSON 字符串, 要求是 object (Qdrant payload 兼容)。
pub fn parse_metadata(raw: Option<String>) -> Result<Option<serde_json::Value>> {
    let Some(text) = raw else {
        return Ok(None);
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).context("`--metadata` must be valid JSON")?;
    if !v.is_object() {
        return Err(anyhow!("`--metadata` must be a JSON object, got: {v}"));
    }
    Ok(Some(v))
}

/// 把 CLI 输入的 ID 字符串解析为 `MemoryId`。
///
/// 走 serde 反序列化, 避免直接依赖 `uuid` crate。
pub fn parse_id(raw: &str) -> Result<MemoryId> {
    let quoted = serde_json::to_string(raw).expect("string is always serializable");
    serde_json::from_str::<MemoryId>(&quoted).with_context(|| format!("invalid memory id: {raw}"))
}

/// UUID 前 8 位 + 省略号, 给列表/搜索结果省横向空间。
fn short_id(id: &MemoryId) -> String {
    let s = id.to_string();
    s.chars().take(8).collect::<String>() + "…"
}

/// `frank memory add` 实现。
pub fn run_add(client: &SyncClient, args: AddArgs) -> Result<()> {
    let scope = scope_of(args.user, args.agent, args.session);
    if scope.is_empty() {
        crate::log::ui::warn("scope is empty; consider --user to avoid global writes");
    }
    let metadata = parse_metadata(args.metadata)?;
    let ids = client.add(&args.content, &scope, metadata.as_ref())?;
    crate::log::ui::success(&format!("stored {} fact(s)", ids.len()));
    for id in &ids {
        println!("  {id}");
    }
    Ok(())
}

/// `frank memory add-raw` 实现。
pub fn run_add_raw(client: &SyncClient, args: AddRawArgs) -> Result<()> {
    let scope = scope_of(args.user, args.agent, args.session);
    let metadata = parse_metadata(args.metadata)?;
    let id = client.add_raw(&args.fact, &scope, metadata.as_ref())?;
    crate::log::ui::success(&format!("stored raw fact: {id}"));
    Ok(())
}

/// `frank memory search` 实现。
pub fn run_search(client: &SyncClient, args: SearchArgs) -> Result<()> {
    let scope = scope_of(args.user, args.agent, None);
    let matches = client.search(&args.query, &scope, args.limit, args.score_threshold)?;

    if matches.is_empty() {
        crate::log::ui::warn("no match");
        return Ok(());
    }

    crate::log::ui::section(&format!("Matches ({} total)", matches.len()));
    for (i, m) in matches.iter().enumerate() {
        println!(
            "  {idx}. [{score:.3}] {id}  {content}",
            idx = i + 1,
            score = m.score,
            id = short_id(&m.record.id),
            content = m.record.content,
        );
    }
    Ok(())
}

/// `frank memory list` 实现。
pub fn run_list(client: &SyncClient, args: ListArgs) -> Result<()> {
    let scope = scope_of(args.user, args.agent, args.session);
    let records = client.list(&scope, args.limit)?;
    if records.is_empty() {
        crate::log::ui::warn("no record in scope");
        return Ok(());
    }
    crate::log::ui::section(&format!("Records ({} total)", records.len()));
    for r in &records {
        println!(
            "  {id}  {created}  {content}",
            id = short_id(&r.id),
            created = r.created_at.format("%Y-%m-%d %H:%M:%S"),
            content = r.content,
        );
    }
    Ok(())
}

/// `frank memory get` 实现。
pub fn run_get(client: &SyncClient, args: GetArgs) -> Result<()> {
    let id = parse_id(&args.id)?;
    match client.get(&id)? {
        None => {
            crate::log::ui::warn(&format!("no record with id {id}"));
        }
        Some(rec) => {
            crate::log::ui::section(&format!("Record {id}"));
            println!("  content    : {}", rec.content);
            println!(
                "  user_id    : {}",
                rec.scope.user_id.unwrap_or_else(|| "-".into())
            );
            println!(
                "  agent_id   : {}",
                rec.scope.agent_id.unwrap_or_else(|| "-".into())
            );
            println!(
                "  session_id : {}",
                rec.scope.session_id.unwrap_or_else(|| "-".into())
            );
            println!("  created_at : {}", rec.created_at);
            println!("  updated_at : {}", rec.updated_at);
            if !rec.metadata.is_null() {
                println!("  metadata   : {}", rec.metadata);
            }
        }
    }
    Ok(())
}

/// `frank memory delete` 实现。
pub fn run_delete(client: &SyncClient, args: DeleteArgs) -> Result<()> {
    let id = parse_id(&args.id)?;
    client.delete(&id)?;
    crate::log::ui::success(&format!("deleted {id}"));
    Ok(())
}

/// `frank memory healthz` 实现。
pub fn run_healthz(client: &SyncClient) -> Result<()> {
    let body = client.healthz()?;
    crate::log::ui::success(&format!("healthz: {}", body.trim()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_of_collects_three_fields() {
        let s = scope_of(
            Some("alice".into()),
            Some("claude".into()),
            Some("sess-1".into()),
        );
        assert_eq!(s.user_id.as_deref(), Some("alice"));
        assert_eq!(s.agent_id.as_deref(), Some("claude"));
        assert_eq!(s.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn parse_metadata_accepts_object() {
        let v = parse_metadata(Some(r#"{"source":"chat"}"#.to_string())).unwrap();
        assert!(v.unwrap().get("source").is_some());
    }

    #[test]
    fn parse_metadata_rejects_array() {
        let err = parse_metadata(Some("[1,2,3]".to_string())).unwrap_err();
        assert!(format!("{err}").contains("must be a JSON object"));
    }

    #[test]
    fn parse_id_rejects_garbage() {
        let err = parse_id("not-a-uuid").unwrap_err();
        assert!(format!("{err:#}").contains("invalid memory id"));
    }
}
