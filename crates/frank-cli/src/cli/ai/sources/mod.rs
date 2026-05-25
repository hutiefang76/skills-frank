//! `frank ai ask --list-models` 的模型清单数据源 (v0.10.8 D1+D2).
//!
//! # 4 路合并思路
//!
//! v0.10.7 写死 `BUILTIN_MODELS` 静态清单, 用户 cc-switch 配的 12 个 provider
//! 一个都看不到. v0.10.8 改成读用户机器上**真实在用**的模型, 4 路按优先级合并:
//!
//! | 路 | 来源 | 例 | 优先级 |
//! |---|---|---|---|
//! | 1 | 各家 CLI 原生配置文件 | `~/.claude/settings.json` 的 `"model"` 字段 | 最高 |
//! | 2 | env vars (临时覆盖) | `ANTHROPIC_MODEL` / `OPENAI_MODEL` / `GEMINI_MODEL` | 中 |
//! | 3 | frank 内置 alias 兜底 | `sonnet, opus, haiku` | 最低 (前 2 全空才显) |
//!
//! cc-switch 等"切换工具"会改各家 CLI 原生配置 — frank **不耦合** cc-switch, 只读
//! 配置结果, 跟谁改的没关系.
//!
//! # 失败处理
//!
//! 每路全 read-only, IO 失败 / parse 失败 → 静默返回空 Vec (调用方 fallback).
//! 这是有意的: 用户没装某家 CLI / 配置文件不存在是**正常情况**, 不该报错.

pub mod claude;
pub mod codex;
pub mod env;
pub mod gemini;
pub mod opencode;

/// 单条模型记录 — 名字 + 来源 (用于 UI 显示 `[配置文件]` / `[env]` / `[内置兜底]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// 模型名 (例 `sonnet`, `gpt-5.5`, `xiaomi/mimo-v2-pro`).
    pub name: String,
    /// 数据源 (决定优先级 + UI 标签).
    pub source: ModelSource,
}

/// 模型来源 — 3 种.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// 来自原生 CLI 配置文件 (例 `~/.claude/settings.json`).
    /// `path` 是文件绝对路径, UI 显示用.
    ConfigFile(std::path::PathBuf),
    /// 来自环境变量 (例 `ANTHROPIC_MODEL`).
    /// `name` 是 env var 名字.
    EnvVar(String),
    /// frank 内置 alias 兜底 (前 2 路全空时用).
    BuiltinAlias,
}

impl ModelSource {
    /// 优先级排序键 — 数字越小越靠前 (ConfigFile=0, EnvVar=1, BuiltinAlias=2).
    ///
    /// 用于去重时保留高优先级 source: 同 name 多次出现, 留 `priority()` 小的.
    #[must_use]
    pub fn priority(&self) -> u8 {
        match self {
            ModelSource::ConfigFile(_) => 0,
            ModelSource::EnvVar(_) => 1,
            ModelSource::BuiltinAlias => 2,
        }
    }

    /// UI 显示用的简短标签 (例 `配置 ~/.claude/settings.json`).
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            ModelSource::ConfigFile(path) => {
                // 把 $HOME 缩成 ~ 让 UI 短点
                let p = path.display().to_string();
                if let Some(home) = dirs::home_dir() {
                    let home_str = home.display().to_string();
                    if let Some(rest) = p.strip_prefix(&home_str) {
                        return format!("配置 ~{rest}");
                    }
                }
                format!("配置 {p}")
            }
            ModelSource::EnvVar(name) => format!("env {name}"),
            ModelSource::BuiltinAlias => "内置兜底".to_string(),
        }
    }
}

/// 4 家原生 CLI 配置文件的内置 alias 兜底 — 前 2 路全空时显示.
///
/// 选名规则: 跟 v0.10.7 `BUILTIN_MODELS` 一致, 走"广泛认可的官方 alias", 不放
/// 实验模型 / 不放具体版本号 (那是 CLI 自己的事).
///
/// **opencode 不兜底** — opencode 完全用户自配 (self-hosted / 任意中转), 没有
/// 官方默认模型名概念. 用户没配 = 显示空, 提示去 `~/.config/opencode/opencode.json` 加.
const BUILTIN_ALIASES: &[(&str, &[&str])] = &[
    ("claude", &["sonnet", "opus", "haiku"]),
    ("codex", &["gpt-5.5", "gpt-5.4"]),
    ("gemini", &["gemini-2.5-pro", "gemini-2.5-flash"]),
    // opencode 无兜底 (用户自配)
];

/// 拿某 provider 的内置 alias 兜底列表.
///
/// 返回 `ModelEntry` 列表 (`source = BuiltinAlias`). opencode → 空 Vec.
fn builtin_aliases(provider: &str) -> Vec<ModelEntry> {
    BUILTIN_ALIASES
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, list)| {
            list.iter()
                .map(|n| ModelEntry {
                    name: (*n).to_string(),
                    source: ModelSource::BuiltinAlias,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 单 provider 收齐所有 model — 配置文件 + env + alias 兜底.
///
/// **顺序**: ConfigFile → EnvVar → BuiltinAlias (D2 用这个顺序去重保高优先).
///
/// 兜底逻辑: 前 2 路全空时**才**加 alias; 前 2 路有任意一个就**不加** alias
/// (用户已经显式配过了, 再加内置就杂乱).
///
/// **未去重未排序** — 调用方拿到原始 3 路拼接结果, 想去重排序走
/// `collect_all_for_provider`. 这个低层 API 给 `refresh-skills` 用 (它有自己的
/// dedupe + max 截断逻辑).
#[must_use]
pub fn collect_user_models(provider: &str) -> Vec<ModelEntry> {
    let mut out = Vec::new();

    // 路 1: 各家 CLI 原生配置文件
    let from_config = match provider {
        "claude" => claude::read_models(),
        "codex" => codex::read_models(),
        "gemini" => gemini::read_models(),
        "opencode" => opencode::read_models(),
        _ => Vec::new(),
    };
    out.extend(from_config);

    // 路 2: env vars (provider → env var 名映射, 详见 env::read_for)
    out.extend(env::read_for(provider));

    // 路 3: 兜底 — 仅当前 2 路全空才显示 alias (避免给用户配过的清单再塞默认)
    if out.is_empty() {
        out.extend(builtin_aliases(provider));
    }

    out
}

/// D2: 4 路合并 + 去重 + 排序 — 单 provider 的最终模型清单.
///
/// # 去重
///
/// 同 name 多次出现保 priority 最小的 (ConfigFile=0 > EnvVar=1 > BuiltinAlias=2).
/// 例: claude 的 `~/.claude/settings.json` 配了 `haiku`, 内置兜底也有 `haiku` —
/// 留前者标 `[配置 ...]`, 不重复显示 `[内置兜底]`.
///
/// # 排序
///
/// 按 `source.priority()` 稳定排序: ConfigFile 在前, EnvVar 中间, BuiltinAlias 在后.
/// 同优先级保留输入顺序 (codex 的顶层 model 在 profiles 前; 多 provider 按 JSON 字典序).
///
/// # 空兜底
///
/// 前 2 路均空时 `collect_raw` 已加 alias 兜底, 所以**只要 builtin_aliases 非空** 这个
/// 函数就不会返回空; opencode 例外 (无兜底) — 用户完全没配时返回空.
#[must_use]
pub fn collect_all_for_provider(provider: &str) -> Vec<ModelEntry> {
    let raw = collect_user_models(provider);

    // 去重: 用 HashMap<name, ModelEntry> 收最高优先级条目.
    // 注意顺序: 后插入的若 priority 更高 (数字小) 才覆盖.
    let mut best: std::collections::HashMap<String, ModelEntry> = std::collections::HashMap::new();
    // 同时维护"首次出现顺序" — 同 priority 时按输入顺序排, HashMap 不保序所以单独存.
    let mut first_seen: Vec<String> = Vec::new();

    for entry in raw {
        let cur_priority = entry.source.priority();
        match best.get(&entry.name) {
            Some(existing) if existing.source.priority() <= cur_priority => {
                // 已有更高 (或同优先级、保先到) → 跳过
            }
            Some(_) => {
                // 已有但当前优先级更高 → 覆盖, 不更新 first_seen (位置已定)
                best.insert(entry.name.clone(), entry);
            }
            None => {
                first_seen.push(entry.name.clone());
                best.insert(entry.name.clone(), entry);
            }
        }
    }

    // 按 (priority asc, first_seen asc) 稳定排序
    let mut order_index: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(first_seen.len());
    for (i, n) in first_seen.iter().enumerate() {
        order_index.insert(n.as_str(), i);
    }

    let mut out: Vec<ModelEntry> = best.into_values().collect();
    out.sort_by_key(|e| {
        (
            e.source.priority(),
            order_index
                .get(e.name.as_str())
                .copied()
                .unwrap_or(usize::MAX),
        )
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_config_first() {
        let cfg = ModelSource::ConfigFile(std::path::PathBuf::from("/x"));
        let env = ModelSource::EnvVar("X".to_string());
        let alias = ModelSource::BuiltinAlias;
        assert!(cfg.priority() < env.priority());
        assert!(env.priority() < alias.priority());
    }

    #[test]
    fn label_shortens_home_dir() {
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".claude").join("settings.json");
            let src = ModelSource::ConfigFile(path);
            let label = src.label();
            assert!(label.starts_with("配置 ~/"), "label={label}");
            assert!(label.contains(".claude"));
        }
    }

    #[test]
    fn label_env_var() {
        let src = ModelSource::EnvVar("ANTHROPIC_MODEL".to_string());
        assert_eq!(src.label(), "env ANTHROPIC_MODEL");
    }

    #[test]
    fn builtin_aliases_4_providers_3_have_fallback() {
        // claude/codex/gemini 有兜底, opencode 没有
        assert!(!builtin_aliases("claude").is_empty());
        assert!(!builtin_aliases("codex").is_empty());
        assert!(!builtin_aliases("gemini").is_empty());
        assert!(builtin_aliases("opencode").is_empty());
        // 未知 provider → 空
        assert!(builtin_aliases("unknown").is_empty());
    }

    #[test]
    fn builtin_aliases_marked_as_builtin_source() {
        for entry in builtin_aliases("claude") {
            assert!(matches!(entry.source, ModelSource::BuiltinAlias));
        }
    }

    #[test]
    fn collect_for_unknown_provider_returns_empty() {
        // 未知 provider 全部 3 路均空 → 空 Vec (不该塞默认)
        // 注意: collect_user_models 内部 builtin 也是空, 所以保险地全空
        let r = collect_user_models("nope-fake-provider");
        assert!(r.is_empty(), "got {r:?}");
    }

    // ===== D2: 去重 + 排序测试 =====

    #[test]
    fn collect_all_dedupes_same_name_keeps_higher_priority() {
        // 手造 raw: 同名 "haiku" 既在 ConfigFile 又在 BuiltinAlias → 应只保 ConfigFile
        // 用 vec 直接喂给排序函数模拟; 不走 collect_raw (它有真 IO)
        let raw = vec![
            ModelEntry {
                name: "haiku".to_string(),
                source: ModelSource::ConfigFile(std::path::PathBuf::from("/tmp/x")),
            },
            ModelEntry {
                name: "haiku".to_string(),
                source: ModelSource::BuiltinAlias,
            },
            ModelEntry {
                name: "opus".to_string(),
                source: ModelSource::BuiltinAlias,
            },
        ];
        let out = dedupe_and_sort_for_test(raw);
        assert_eq!(out.len(), 2, "haiku 去重后只剩 1 条 + opus 1 条 = 2");
        let haiku = out.iter().find(|e| e.name == "haiku").unwrap();
        assert!(
            matches!(haiku.source, ModelSource::ConfigFile(_)),
            "去重保留 ConfigFile, 不是 alias"
        );
    }

    #[test]
    fn collect_all_sorts_config_before_alias() {
        let raw = vec![
            ModelEntry {
                name: "alias-a".to_string(),
                source: ModelSource::BuiltinAlias,
            },
            ModelEntry {
                name: "config-b".to_string(),
                source: ModelSource::ConfigFile(std::path::PathBuf::from("/x")),
            },
            ModelEntry {
                name: "env-c".to_string(),
                source: ModelSource::EnvVar("X_MODEL".to_string()),
            },
        ];
        let out = dedupe_and_sort_for_test(raw);
        // 排序: config 优先 0, env 1, alias 2
        assert_eq!(out[0].name, "config-b");
        assert_eq!(out[1].name, "env-c");
        assert_eq!(out[2].name, "alias-a");
    }

    #[test]
    fn collect_all_preserves_input_order_within_same_priority() {
        // 同 priority (都 ConfigFile) → 按输入顺序保留
        let path = std::path::PathBuf::from("/x");
        let raw = vec![
            ModelEntry {
                name: "first".to_string(),
                source: ModelSource::ConfigFile(path.clone()),
            },
            ModelEntry {
                name: "second".to_string(),
                source: ModelSource::ConfigFile(path.clone()),
            },
            ModelEntry {
                name: "third".to_string(),
                source: ModelSource::ConfigFile(path),
            },
        ];
        let out = dedupe_and_sort_for_test(raw);
        assert_eq!(out[0].name, "first");
        assert_eq!(out[1].name, "second");
        assert_eq!(out[2].name, "third");
    }

    /// 测试辅助 — 复用 `collect_all_for_provider` 排序逻辑, 但跳过真 IO.
    ///
    /// 把 collect_all_for_provider 的排序段抽出来给测试用; 真生产路径还是走主函数.
    fn dedupe_and_sort_for_test(raw: Vec<ModelEntry>) -> Vec<ModelEntry> {
        let mut best: std::collections::HashMap<String, ModelEntry> =
            std::collections::HashMap::new();
        let mut first_seen: Vec<String> = Vec::new();
        for entry in raw {
            let cur = entry.source.priority();
            match best.get(&entry.name) {
                Some(e) if e.source.priority() <= cur => {}
                Some(_) => {
                    best.insert(entry.name.clone(), entry);
                }
                None => {
                    first_seen.push(entry.name.clone());
                    best.insert(entry.name.clone(), entry);
                }
            }
        }
        let mut idx: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::with_capacity(first_seen.len());
        for (i, n) in first_seen.iter().enumerate() {
            idx.insert(n.as_str(), i);
        }
        let mut out: Vec<ModelEntry> = best.into_values().collect();
        out.sort_by_key(|e| {
            (
                e.source.priority(),
                idx.get(e.name.as_str()).copied().unwrap_or(usize::MAX),
            )
        });
        out
    }

    #[test]
    fn collect_all_for_provider_real_path_returns_non_empty_for_known() {
        // 跑真 collect_all_for_provider — claude 至少有内置 alias, 不会空
        // (注意可能受真实 HOME 影响, 所以只断言"有东西", 不断言具体内容)
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let r = collect_all_for_provider("claude");
        assert!(!r.is_empty(), "claude 至少有内置 alias 兜底");
    }
}
