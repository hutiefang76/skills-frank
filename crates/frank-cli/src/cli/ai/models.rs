//! `frank ai ask --list-models` — 列出 4 家 CLI 当前能用的模型 (v0.10.8).
//!
//! # v0.10.8 跟 v0.10.7 的区别
//!
//! v0.10.7 写死 BUILTIN_MODELS 静态清单, 用户 cc-switch 配的 12 个 provider 看不到.
//! v0.10.8 改读真用户机器配置 — 调 `sources::collect_all_for_provider` 拿 4 路合并
//! (配置文件 + env + 兜底 alias) 的最终清单, 每条带 source 标签.
//!
//! # 跟 Web UI 路径合一
//!
//! `orchestrator_server::detect_models` 也调同一个 `sources::collect_all_for_provider`,
//! 所以浏览器下拉跟 CLI `--list-models` 永远显示一样的清单 — 不会因为两边代码不
//! 同步而漂.
//!
//! # 输出格式 (UI 改进, 大白话)
//!
//! 按 source 分组显示 — 用户一眼看出每个 model 哪来的:
//!
//! ```text
//! claude:
//!   haiku                           [配置 ~/.claude/settings.json]
//!   sonnet, opus                    [内置兜底]
//! codex:
//!   gpt-5.5                         [配置 ~/.codex/config.toml]
//!   gpt-5.4                         [内置兜底]
//! ...
//! ```

use anyhow::Result;

use super::sources::{self, ModelEntry};

/// 4 家 provider 的固定显示顺序 (跟 v0.10.7 一致, claude 总在最前).
const PROVIDER_ORDER: &[&str] = &["claude", "codex", "opencode", "gemini"];

/// 4 家 CLI 的 binary 名 (跟 `invocation()` 保持一致, 用来 which 检测装没装).
const CLI_BINS: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("gemini", "gemini"),
];

/// 拿 provider 对应的 binary 名 (大部分跟 provider 同名).
fn bin_for(provider: &str) -> &str {
    CLI_BINS
        .iter()
        .find(|(p, _)| *p == provider)
        .map_or(provider, |(_, b)| *b)
}

/// 单 provider 的模型清单 (CLI 装没装 + 已去重排序的 ModelEntry 列表).
#[derive(Debug, Clone)]
pub struct ProviderModels {
    /// 模型条目 (含 name + source, 已按 source 优先级排序).
    pub entries: Vec<ModelEntry>,
    /// CLI binary 是否装了 (false 时打印用 "未装" 提示).
    pub installed: bool,
}

/// 给 CLI / Web UI 用的总入口 — 4 家全收齐.
///
/// 返回 `Vec<(provider, ProviderModels)>` (固定顺序 claude → codex → opencode → gemini).
/// 每个 ProviderModels 已经走过 `sources::collect_all_for_provider` 的去重 + 排序.
#[must_use]
pub fn list_all_for_cli() -> Vec<(String, ProviderModels)> {
    PROVIDER_ORDER
        .iter()
        .map(|p| {
            let installed = which::which(bin_for(p)).is_ok();
            let entries = sources::collect_all_for_provider(p);
            ((*p).to_string(), ProviderModels { entries, installed })
        })
        .collect()
}

/// 把所有 provider 的模型清单 print 到 stdout, 按 source 分组.
///
/// 输出格式 (v0.10.8 改):
/// ```text
/// 可用模型 (frank ai ask --to <provider> --model <name>)
///
/// claude:
///   haiku                           [配置 ~/.claude/settings.json]
///   sonnet, opus, haiku             [内置兜底]
///
/// codex:
///   gpt-5.5                         [配置 ~/.codex/config.toml]
///   ...
///
/// (未装的 CLI 标 "⚠ 未装")
/// ```
pub fn print_all() -> Result<()> {
    let all = list_all_for_cli();
    crate::log::ui::section("可用模型 (frank ai ask --to <provider> --model <name>)");

    for (provider, pm) in &all {
        if !pm.installed {
            // 未装时只提示装哪个 binary, 不列内置 (装好用户再跑一次就拿到真清单)
            let bin = bin_for(provider);
            println!(
                "{:<10} ⚠ 未装, 跑 `brew install {bin}` 装一下",
                format!("{provider}:")
            );
            continue;
        }

        if pm.entries.is_empty() {
            // 装了但啥都没配 (opencode 典型) — 提示去哪里加
            let bin = bin_for(provider);
            println!(
                "{:<10} (没配 model, 跑 `{bin} models` 或编辑配置加一个)",
                format!("{provider}:")
            );
            continue;
        }

        // 按 source 分组打 — 同组的 name 用 ", " 合并一行
        println!("{provider}:");
        for line in group_by_source(&pm.entries) {
            // 模型名占 30 列, source 标签 [.] 右贴
            println!("  {:<30} [{}]", line.names, line.label);
        }
    }
    Ok(())
}

/// 同 source label 的模型名合一行 (UI 简洁).
struct GroupedLine {
    /// 同 source 的 model name, 逗号分隔.
    names: String,
    /// source 标签 (例 `配置 ~/.claude/settings.json`).
    label: String,
}

/// 把 ModelEntry 按 source.label() 分组, 保留首次出现顺序.
///
/// 例: `[(haiku, Config), (sonnet, Alias), (opus, Alias)]` →
///     `[(haiku, "配置 ..."), (sonnet, opus, "内置兜底")]`.
fn group_by_source(entries: &[ModelEntry]) -> Vec<GroupedLine> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for e in entries {
        let label = e.source.label();
        if let Some(g) = groups.iter_mut().find(|(l, _)| *l == label) {
            g.1.push(e.name.clone());
        } else {
            groups.push((label, vec![e.name.clone()]));
        }
    }
    groups
        .into_iter()
        .map(|(label, names)| GroupedLine {
            names: names.join(", "),
            label,
        })
        .collect()
}

/// Web UI 路径用的: 拿某 provider 的纯 model 名清单 (不带 source 标签).
///
/// 给 `orchestrator_server::detect_models` 用 — UI 下拉只要 string list,
/// 不需要 source 标签 (那是 CLI 的 UX, 下拉里塞标签反而乱).
///
/// 跟 CLI 路径调同一个 `collect_all_for_provider` 拿一致清单.
#[must_use]
pub fn detect_models_for_ui(provider: &str) -> Vec<String> {
    sources::collect_all_for_provider(provider)
        .into_iter()
        .map(|e| e.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ai::sources::ModelSource;

    #[test]
    fn list_all_returns_4_providers() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let all = list_all_for_cli();
        assert_eq!(all.len(), 4);
        let names: Vec<&str> = all.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(names, ["claude", "codex", "opencode", "gemini"]);
    }

    #[test]
    fn list_all_keeps_claude_first() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let all = list_all_for_cli();
        assert_eq!(all[0].0, "claude");
    }

    #[test]
    fn detect_for_ui_returns_strings_only() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        // claude 至少有内置 alias 兜底 → 非空
        let names = detect_models_for_ui("claude");
        assert!(!names.is_empty());
        // 全是 string, 不该有 source 标签 (调用方 UI 自己加)
        for n in &names {
            assert!(!n.contains('['), "name={n} 不该含 [...] 标签");
        }
    }

    #[test]
    fn detect_for_ui_empty_for_unknown() {
        // 未知 provider → 空 (无内置兜底也无配置)
        assert!(detect_models_for_ui("unknown-fake").is_empty());
    }

    #[test]
    fn group_by_source_same_label_merges() {
        // 同 BuiltinAlias 标签的 3 个 model 合一行
        let entries = vec![
            ModelEntry {
                name: "a".to_string(),
                source: ModelSource::BuiltinAlias,
            },
            ModelEntry {
                name: "b".to_string(),
                source: ModelSource::BuiltinAlias,
            },
            ModelEntry {
                name: "c".to_string(),
                source: ModelSource::BuiltinAlias,
            },
        ];
        let groups = group_by_source(&entries);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].names, "a, b, c");
        assert_eq!(groups[0].label, "内置兜底");
    }

    #[test]
    fn group_by_source_diff_label_split() {
        let entries = vec![
            ModelEntry {
                name: "x".to_string(),
                source: ModelSource::ConfigFile(std::path::PathBuf::from("/x")),
            },
            ModelEntry {
                name: "y".to_string(),
                source: ModelSource::BuiltinAlias,
            },
        ];
        let groups = group_by_source(&entries);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].names, "x"); // ConfigFile 先
        assert_eq!(groups[1].names, "y"); // Alias 后
    }

    #[test]
    fn print_all_runs_no_panic() {
        // 真路径跑一次, 不爬 (不验输出内容 — 因为依赖真实 HOME)
        let _lock = crate::cli::ai::history_store::test_home_lock();
        print_all().unwrap();
    }

    #[test]
    fn bin_for_unknown_provider_returns_provider() {
        // 未知 provider → 直接拿 provider 本身当 binary 名 (fallback)
        assert_eq!(bin_for("nope"), "nope");
    }

    #[test]
    fn bin_for_known_providers() {
        assert_eq!(bin_for("claude"), "claude");
        assert_eq!(bin_for("codex"), "codex");
        assert_eq!(bin_for("opencode"), "opencode");
        assert_eq!(bin_for("gemini"), "gemini");
    }
}
