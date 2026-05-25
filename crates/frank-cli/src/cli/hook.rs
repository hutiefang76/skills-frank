//! `frank hook` — Claude Code PostToolUse hook 桥接 (v0.11 子项 H)。
//!
//! 让 Claude Code 用户**零成本**双写 mcp__memory 数据到 frank-memory:
//! 用户继续用 mcp__memory (Anthropic 官方), frank 默默把同样的数据存到本地
//! LanceDB + 远程 sync-agent, 实现 POSITION.md "比 mcp_memory 强 + 兼容" 路径。
//!
//! # 子命令
//!
//! - `frank hook install`     注册到 ~/.claude/settings.json hooks.PostToolUse
//! - `frank hook uninstall`   反向移除
//! - `frank hook handle`      被 Claude Code 调用 — 读 stdin JSON, 派发到 frank memory
//! - `frank hook status`      看是否已装, settings.json 里的 entry 是啥
//!
//! # Claude Code hook 协议 (PostToolUse)
//!
//! Claude Code 在每次工具调用结束后会 (按 matcher) 触发 hook:
//! 1. 通过 stdin 传一段 JSON: `{ session_id, tool_name, tool_input, tool_response }`
//! 2. 期望 hook 程序读完 stdin, 处理, 0 exit code 表示成功
//! 3. stdout / stderr 不会影响 Claude Code (但会出现在 hook 日志里, debug 用)
//!
//! frank hook handle 关注 `tool_name == "mcp__memory__add_observations"`,
//! 把 `tool_input.observations[].contents[]` 拆条转 `frank memory add_raw`.
//!
//! 失败永远不阻断用户的 mcp_memory 体验 — handle 内部任何错误都吞掉, exit 0。

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

/// `frank hook` 顶层参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: HookCommand,
}

/// 子命令清单。
#[derive(Subcommand, Debug)]
pub enum HookCommand {
    /// 注册 frank-hook 到 ~/.claude/settings.json hooks.PostToolUse.
    Install,
    /// 反向移除 (留其他 hook 不动).
    Uninstall,
    /// 看 hook 安装状态 (是否注册 / 配的 matcher + command).
    Status,
    /// 被 Claude Code 调用的处理器 — 读 stdin JSON 派发到 frank memory.
    /// 用户一般不直接调; settings.json 里的 command 字段指向这里.
    Handle,
}

/// 派发器。
pub fn run(args: Args) -> Result<()> {
    match args.command {
        HookCommand::Install => install_hook(),
        HookCommand::Uninstall => uninstall_hook(),
        HookCommand::Status => show_status(),
        HookCommand::Handle => handle_event(),
    }
}

/// ~/.claude/settings.json 路径。
fn settings_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".claude")
        .join("settings.json"))
}

/// 我们要注的 matcher: 匹配所有 mcp__memory__* 工具.
/// Claude Code 用 regex/glob 匹配, 这里 `.*` 兜底.
const HOOK_MATCHER: &str = "mcp__memory__.*";
/// 我们注册的 command — 自己调自己, 用绝对 frank 路径避免 PATH 问题.
const HOOK_COMMAND: &str = "frank hook handle";

fn install_hook() -> Result<()> {
    let path = settings_path()?;
    if !path.exists() {
        // 首次配 Claude Code 时该文件应该已存在; 不在就提示用户先跑一次 claude
        crate::log::ui::warn(&format!(
            "{} 不存在 — 先跑一次 `claude` 让它创建, 再 frank hook install",
            path.display()
        ));
        return Ok(());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut root: Value = serde_json::from_str(&text).context("parse settings.json")?;

    let root_obj = root
        .as_object_mut()
        .context("settings.json root 不是 object")?;
    let hooks = root_obj
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .context("hooks 不是 object")?;
    let post_tool_use = hooks
        .entry("PostToolUse".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .context("PostToolUse 不是 array")?;

    // 已存在? 看 matcher == HOOK_MATCHER 且 command == HOOK_COMMAND
    let already = post_tool_use.iter().any(|entry| {
        entry
            .get("matcher")
            .and_then(Value::as_str)
            .is_some_and(|m| m == HOOK_MATCHER)
            && entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|arr| {
                    arr.iter().any(|h| {
                        h.get("command").and_then(Value::as_str).is_some_and(|c| c == HOOK_COMMAND)
                    })
                })
    });
    if already {
        crate::log::ui::info("frank hook 已注册, 跳过");
        return Ok(());
    }

    let entry = serde_json::json!({
        "matcher": HOOK_MATCHER,
        "hooks": [
            { "type": "command", "command": HOOK_COMMAND }
        ]
    });
    post_tool_use.push(entry);

    let new_text =
        serde_json::to_string_pretty(&root).context("serialize updated settings.json")?;
    fs::write(&path, new_text).with_context(|| format!("write {}", path.display()))?;
    crate::log::ui::success(&format!(
        "frank hook 已注册到 {} (matcher={HOOK_MATCHER})",
        path.display()
    ));

    // v0.12.0 G: 同时注入 ~/.claude/CLAUDE.md, 让 Claude 知道有 frank-memory.
    inject_claude_md()?;

    crate::log::ui::info("以后 Claude Code 调 mcp__memory__add_observations 时, frank 会自动双写到本地 LanceDB + 远程 sync-agent");
    Ok(())
}

/// v0.12.0 G: 把 frank-memory 介绍段注入到 ~/.claude/CLAUDE.md (无则建). 幂等, 用 BEGIN/END 标记定位.
fn inject_claude_md() -> Result<()> {
    let Some(home) = dirs::home_dir() else {
        return Ok(()); // 非家目录场景跳过
    };
    let path = home.join(".claude").join("CLAUDE.md");
    let begin = "<!-- BEGIN frank-memory (managed by `frank hook install`) -->";
    let end = "<!-- END frank-memory -->";
    let block = format!(
        "\n{begin}\n## frank-memory (v0.12.0)\n\n\
        你有访问用户分布式记忆的能力 (frank-memory):\n\
        - 查历史: `frank memory search \"<query>\" --limit 5`\n\
        - 存新事: `frank memory add \"<内容>\"` (会自动抽 fact)\n\
        - 列出最近: `frank memory list --limit 10`\n\
        - 看 quota / 删除状态: `frank tenant status`\n\n\
        数据隔离: 每个 token sha256 派生独立 tenant, 用户数据互不可见.\n\
        Server: frank.hutiefang.com (用户也可自建, `frank config set sync.agent_url ...`).\n\
        {end}\n"
    );

    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.contains(begin) {
        crate::log::ui::info("CLAUDE.md 已注入过 frank-memory 段, 跳过");
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let combined = if existing.trim().is_empty() {
        block
    } else {
        format!("{existing}\n{block}")
    };
    fs::write(&path, combined).with_context(|| format!("write {}", path.display()))?;
    crate::log::ui::success(&format!("CLAUDE.md 已注入 frank-memory 段 ({})", path.display()));
    Ok(())
}

/// v0.12.0 G: 反向清 CLAUDE.md 中 frank-memory 段 (uninstall 用).
fn purge_claude_md() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let path = home.join(".claude").join("CLAUDE.md");
    let Ok(existing) = fs::read_to_string(&path) else {
        return;
    };
    let begin = "<!-- BEGIN frank-memory";
    let end = "<!-- END frank-memory -->";
    let (Some(start), Some(end_idx)) = (existing.find(begin), existing.find(end)) else {
        return; // 没标记, 跳过
    };
    let after_end = end_idx + end.len();
    let mut new_text = String::with_capacity(existing.len());
    new_text.push_str(&existing[..start]);
    new_text.push_str(&existing[after_end..]);
    // 清掉孤立的换行
    let new_text = new_text.replace("\n\n\n", "\n\n");
    if fs::write(&path, new_text).is_ok() {
        crate::log::ui::info("CLAUDE.md 中的 frank-memory 段已清掉");
    }
}

fn uninstall_hook() -> Result<()> {
    let path = settings_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!("{} 不存在, 没东西可删", path.display()));
        return Ok(());
    }
    let text = fs::read_to_string(&path).context("read settings.json")?;
    let mut root: Value = serde_json::from_str(&text).context("parse settings.json")?;

    let Some(post_tool_use) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
        .and_then(|h| h.get_mut("PostToolUse"))
        .and_then(|p| p.as_array_mut())
    else {
        crate::log::ui::warn("hooks.PostToolUse 不存在");
        return Ok(());
    };
    let before = post_tool_use.len();
    post_tool_use.retain(|entry| {
        let is_frank = entry
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|arr| {
                arr.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c == HOOK_COMMAND)
                })
            });
        !is_frank
    });
    let removed = before - post_tool_use.len();
    if removed == 0 {
        crate::log::ui::info("没找到 frank hook entry, 跳过");
        return Ok(());
    }

    let new_text = serde_json::to_string_pretty(&root).context("serialize settings.json")?;
    fs::write(&path, new_text).context("write settings.json")?;
    crate::log::ui::success(&format!("已删 {removed} 条 frank hook entry"));
    // v0.12.0 G: 同时清 CLAUDE.md 注入段
    purge_claude_md();
    Ok(())
}

fn show_status() -> Result<()> {
    let path = settings_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!("{} 不存在", path.display()));
        return Ok(());
    }
    let text = fs::read_to_string(&path).context("read settings.json")?;
    let root: Value = serde_json::from_str(&text).context("parse settings.json")?;
    let post_tool_use = root
        .pointer("/hooks/PostToolUse")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    crate::log::ui::section("frank hook 状态");
    let frank_entries: Vec<_> = post_tool_use
        .iter()
        .filter(|e| {
            e.get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|arr| {
                    arr.iter().any(|h| {
                        h.get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|c| c == HOOK_COMMAND)
                    })
                })
        })
        .collect();
    if frank_entries.is_empty() {
        crate::log::ui::warn("未注册. 跑 `frank hook install` 注册");
    } else {
        crate::log::ui::success(&format!("已注册 {} 条", frank_entries.len()));
        for (i, entry) in frank_entries.iter().enumerate() {
            let matcher = entry
                .get("matcher")
                .and_then(Value::as_str)
                .unwrap_or("?");
            println!("  [{i}] matcher={matcher}");
        }
    }
    crate::log::ui::info(&format!("settings.json: {}", path.display()));
    Ok(())
}

/// Claude Code 调用入口 — 读 stdin JSON, 派发到 frank memory.
///
/// 永远 exit 0 (失败不阻断 Claude Code). 错误 log 到 stderr 给 debug.
fn handle_event() -> Result<()> {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        // 没 stdin 也 OK; 直接静默 exit
        return Ok(());
    }
    if buf.trim().is_empty() {
        return Ok(());
    }

    let Ok(payload) = serde_json::from_str::<Value>(&buf) else {
        tracing::warn!(payload = %buf, "hook handle: 收到非法 JSON, 忽略");
        return Ok(());
    };

    let Some(tool_name) = payload.get("tool_name").and_then(Value::as_str) else {
        tracing::debug!("hook handle: 没 tool_name 字段");
        return Ok(());
    };
    if tool_name != "mcp__memory__add_observations" {
        tracing::debug!(tool_name, "hook handle: 非 add_observations, 跳");
        return Ok(());
    }

    let Some(observations) = payload
        .pointer("/tool_input/observations")
        .and_then(Value::as_array)
    else {
        tracing::debug!("hook handle: tool_input.observations 不存在");
        return Ok(());
    };

    // 提取所有 contents string, 拼成 (entityName, content) pairs
    let mut to_save: Vec<(String, String)> = Vec::new();
    for obs in observations {
        let entity = obs
            .get("entityName")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if let Some(contents) = obs.get("contents").and_then(Value::as_array) {
            for c in contents {
                if let Some(s) = c.as_str() {
                    if !s.trim().is_empty() {
                        to_save.push((entity.clone(), s.to_string()));
                    }
                }
            }
        }
    }
    if to_save.is_empty() {
        return Ok(());
    }

    // 转发到 frank-sync-agent (best-effort, 失败吞掉不阻断 Claude Code)
    forward_to_sync_agent(&payload, &to_save);
    Ok(())
}

/// 同步调 sync_client::add_raw 转发每条 fact.
/// 失败只 log 不报错 (PostToolUse hook 不该阻断 Claude Code).
fn forward_to_sync_agent(payload: &Value, items: &[(String, String)]) {
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("claude-hook");

    let client = match crate::sync_client::SyncClient::from_env_or_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error=?e, "hook handle: 无法建 sync_client, 跳过转发");
            return;
        }
    };

    let scope = frank_memory::Scope {
        user_id: std::env::var("USER").ok(),
        agent_id: Some("claude-code".to_string()),
        session_id: Some(session_id.to_string()),
    };

    let mut ok = 0_usize;
    let mut fail = 0_usize;
    for (entity, content) in items {
        let fact = format!("{entity}: {content}");
        let meta = serde_json::json!({
            "source": "mcp__memory__add_observations",
            "entity": entity,
        });
        match client.add_raw(&fact, &scope, Some(&meta)) {
            Ok(_) => ok += 1,
            Err(e) => {
                tracing::warn!(error=?e, fact, "hook handle: add_raw 失败");
                fail += 1;
            }
        }
    }
    tracing::info!(ok, fail, "hook handle: forwarded to sync-agent");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// settings.json 路径解析包含 ~/.claude/settings.json
    #[test]
    fn settings_path_under_dot_claude() {
        let p = settings_path().expect("settings_path");
        let s = p.to_string_lossy();
        assert!(s.contains(".claude"));
        assert!(s.ends_with("settings.json"));
    }

    /// HOOK_MATCHER 格式合理 (mcp__memory__.*).
    #[test]
    fn matcher_pattern_format() {
        assert_eq!(HOOK_MATCHER, "mcp__memory__.*");
        // 验证 prefix 匹配可期 (claude code 用 regex)
        assert!("mcp__memory__add_observations".starts_with("mcp__memory__"));
        assert!("mcp__memory__create_entities".starts_with("mcp__memory__"));
        assert!(!"Bash".starts_with("mcp__memory__"));
        assert!(!"mcp__filesystem__read".starts_with("mcp__memory__"));
    }

    /// install_hook 二次调用幂等 (用临时 settings.json).
    /// 注: 真测要 mock home dir; 这里只验 matcher 检测逻辑.
    #[test]
    fn hook_already_registered_detected() {
        let entry = serde_json::json!({
            "matcher": HOOK_MATCHER,
            "hooks": [{"type": "command", "command": HOOK_COMMAND}]
        });
        let arr = [entry.clone()];
        let already = arr.iter().any(|e| {
            e.get("matcher")
                .and_then(Value::as_str)
                .is_some_and(|m| m == HOOK_MATCHER)
                && e.get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(|c| c == HOOK_COMMAND)
                        })
                    })
        });
        assert!(already);
    }
}
