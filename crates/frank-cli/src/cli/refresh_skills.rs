//! `frank refresh-skills` — 按用户配的 model 自动生成 slash command skill。
//!
//! # 干啥
//!
//! v0.10.8 之前 frank 只有 5 个写死的 slash command (`/frank-ask-claude` 等),
//! 用模型也是写死的 (claude CLI 自家配的默认). 用户在 cc-switch 配了 5 个不同
//! 模型 (官方 sonnet, 中转站 kimi-k2.5, zkeys 免费 ...) 想用 slash command 切
//! 模型 — 之前只能跟 prompt 里说 "用 kimi 答" 让 frank 转 `--model kimi-k2.5`.
//!
//! v0.10.8 之后:
//!
//! ```text
//! $ frank refresh-skills
//! → 检测 claude 配的 model: sonnet, opus, haiku, kimi-k2.5, gpt-5.4
//! → 写 ~/.claude/skills/frank-ask-claude-sonnet/SKILL.md
//! → 写 ~/.claude/skills/frank-ask-claude-opus/SKILL.md
//! → 写 ~/.claude/skills/frank-ask-claude-kimi-k2-5/SKILL.md
//! ...
//! ```
//!
//! 用户在 claude session 输入 `/frank-ask-claude-kimi-k2-5 你好` 触发该 skill,
//! 它告诉 claude 跑 `frank ai ask --to claude --model kimi-k2.5 "你好"`.
//!
//! # 命令面
//!
//! - `frank refresh-skills` — 默认每 provider 取**前 5 个**最高优先级 model 生成
//! - `--max N` — 改上限 (例 `--max 10`)
//! - `--all` — 不限上限, 全部生成 (warning: 可能装一堆)
//! - `--dry-run` — 只显示会生成/删哪些, 不实际写
//! - `--clean-only` — 只删过期, 不生成新
//!
//! # install/scan 钩子
//!
//! `auto_refresh(silent=true)` 由 `cli::install` / `cli::scan` 末尾调一次 —
//! 用户装 / 扫之后顺手刷, 不打印任何东西 (失败也吞), 不让 refresh 拖累主流程.

use anyhow::Result;
use clap::Parser;

use crate::cli::ai::skill_gen::{self, SkillTemplate};
use crate::cli::ai::sources;

/// 默认每个 provider 取前 N 个 model 生成 skill (太多看着乱).
const DEFAULT_MAX_PER_PROVIDER: usize = 5;

/// 4 家 provider — refresh 时全跑一遍 (没配 = 拿不到 model = 该 provider 跳过).
const PROVIDERS: &[&str] = &["claude", "codex", "gemini", "opencode"];

/// `frank refresh-skills` 参数.
#[derive(Parser, Debug)]
pub struct Args {
    /// 每个 provider 最多生成几个 skill (默认 5).
    #[arg(long)]
    pub max: Option<usize>,

    /// 不限上限, 全部 model 都生成 skill (跟 `--max` 互斥).
    #[arg(long, conflicts_with = "max")]
    pub all: bool,

    /// 干跑 — 只显示会生成/删哪些, 不实际写文件.
    #[arg(long)]
    pub dry_run: bool,

    /// 只删过期 skill, 不生成新.
    #[arg(long)]
    pub clean_only: bool,
}

/// 执行 refresh-skills 命令.
pub fn run(args: Args) -> Result<()> {
    let max = if args.all {
        usize::MAX
    } else {
        args.max.unwrap_or(DEFAULT_MAX_PER_PROVIDER)
    };
    let report = refresh(max, args.dry_run, args.clean_only)?;
    print_report(&report, args.dry_run);
    Ok(())
}

/// 自动 refresh — 给 install/scan 钩子用, 静默版.
///
/// `silent=true`: 不打印任何东西 (UI 也不更新), 失败也吞 (仅 tracing::warn).
/// `silent=false`: 跟 `frank refresh-skills` 一样, 打 section + 列删/加.
pub fn auto_refresh(silent: bool) {
    match refresh(DEFAULT_MAX_PER_PROVIDER, false, false) {
        Ok(report) => {
            if !silent {
                print_report(&report, false);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "refresh-skills (auto) failed; ignored");
        }
    }
}

/// 单 provider 的 refresh 结果.
#[derive(Debug, Default)]
pub struct ProviderReport {
    /// provider 名 (例 `claude`).
    pub provider: String,
    /// 本次生成 / 已存在保留的 skill 名列表.
    pub kept: Vec<String>,
    /// 本次清掉的 stale skill 名列表.
    pub removed: Vec<String>,
    /// 跳过原因 (例 没找到任何 model). 空 string 表示没跳过.
    pub skipped: String,
}

/// 全部 provider 的 refresh 报告.
#[derive(Debug, Default)]
pub struct RefreshReport {
    /// 每个 provider 一个子报告.
    pub providers: Vec<ProviderReport>,
}

/// 主 refresh 流程 — 对 4 家 provider 各跑一遍.
fn refresh(max: usize, dry_run: bool, clean_only: bool) -> Result<RefreshReport> {
    let mut report = RefreshReport::default();
    for &provider in PROVIDERS {
        let pr = refresh_one(provider, max, dry_run, clean_only)?;
        report.providers.push(pr);
    }
    Ok(report)
}

/// 单 provider 的 refresh — 拉用户配的 model, 截断到 max, 写 skill + 清 stale.
fn refresh_one(
    provider: &str,
    max: usize,
    dry_run: bool,
    clean_only: bool,
) -> Result<ProviderReport> {
    let mut pr = ProviderReport {
        provider: provider.to_string(),
        ..Default::default()
    };

    // 拿 skills 目录 — 4 家都进 ~/.claude/skills/ (slash command 在 claude session 触发).
    let Some(target_dir) = skill_gen::skills_dir_for(provider) else {
        pr.skipped = format!("不识别 provider {provider}");
        return Ok(pr);
    };

    // 拉用户配的 model — Worker A 的 sources::collect_user_models.
    // 已按优先级排序 (ConfigFile 在前, EnvVar 中, BuiltinAlias 后).
    let entries = sources::collect_user_models(provider);
    if entries.is_empty() {
        pr.skipped = "用户没配任何 model (CLI 没装 / 配置空 / 兜底也空)".to_string();
        // 没 model 也跑一遍 clean — 用户可能从有 model 变成没 model, 旧 skill 该清
        let removed = skill_gen::clean_stale_skills(&target_dir, provider, &[])?;
        pr.removed = removed;
        return Ok(pr);
    }

    // 去重保留最高优先级 source (同 name 多次出现, 留 priority 小的 = 配置文件 > env > 兜底).
    let dedup = dedupe_by_priority(&entries);
    // 截断到 max
    let kept_models: Vec<String> = dedup.iter().take(max).map(|m| m.name.clone()).collect();

    // 清 stale — 不在 kept_models 里的 frank-ask-<provider>-* 都删
    let removed = if dry_run {
        // 干跑: 拟算会删哪些
        compute_stale_skills_dry(&target_dir, provider, &kept_models)
    } else {
        skill_gen::clean_stale_skills(&target_dir, provider, &kept_models)?
    };
    pr.removed = removed;

    if clean_only {
        // 只清不生成 — 把 kept_models 全部回滚成"已存在保留"展示
        pr.kept = kept_models
            .iter()
            .map(|m| format!("frank-ask-{provider}-{}", skill_gen::safe_model_name(m)))
            .collect();
        return Ok(pr);
    }

    // 生成
    for model in &kept_models {
        let tpl = SkillTemplate {
            provider: provider.to_string(),
            model: model.clone(),
            target_dir: target_dir.clone(),
        };
        if !dry_run {
            skill_gen::write_skill(&tpl)?;
        }
        pr.kept.push(tpl.skill_name());
    }

    Ok(pr)
}

/// dry-run 时算"会删哪些"但不实际删 — 跟 `clean_stale_skills` 同逻辑只是只读.
fn compute_stale_skills_dry(
    target_dir: &std::path::Path,
    provider: &str,
    current_models: &[String],
) -> Vec<String> {
    let keep: std::collections::HashSet<String> = current_models
        .iter()
        .map(|m| format!("frank-ask-{provider}-{}", skill_gen::safe_model_name(m)))
        .collect();
    let prefix = format!("frank-ask-{provider}-");
    let Ok(rd) = std::fs::read_dir(target_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str().map(String::from) {
            if name.starts_with(&prefix) && !keep.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// 同 name 多次出现时只留 priority 最小的 (按 source 优先级去重).
fn dedupe_by_priority(entries: &[sources::ModelEntry]) -> Vec<sources::ModelEntry> {
    let mut best: std::collections::HashMap<String, sources::ModelEntry> =
        std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in entries {
        match best.get(&e.name) {
            None => {
                best.insert(e.name.clone(), e.clone());
                order.push(e.name.clone());
            }
            Some(existing) if e.source.priority() < existing.source.priority() => {
                best.insert(e.name.clone(), e.clone());
            }
            _ => {}
        }
    }
    order.into_iter().filter_map(|n| best.remove(&n)).collect()
}

/// 打印 refresh 报告.
fn print_report(report: &RefreshReport, dry_run: bool) {
    let title = if dry_run {
        "refresh-skills (干跑, 不写)"
    } else {
        "refresh-skills"
    };
    crate::log::ui::section(title);
    let mut total_kept = 0;
    let mut total_removed = 0;
    for pr in &report.providers {
        if !pr.skipped.is_empty() {
            crate::log::ui::info(&format!("  {}: 跳过 — {}", pr.provider, pr.skipped));
            // 即便跳过, 如果还清了点东西也要打出来
            if !pr.removed.is_empty() {
                total_removed += pr.removed.len();
                crate::log::ui::info(&format!(
                    "  {}: 清掉过期 {} 个 ({})",
                    pr.provider,
                    pr.removed.len(),
                    pr.removed.join(", ")
                ));
            }
            continue;
        }
        total_kept += pr.kept.len();
        total_removed += pr.removed.len();
        crate::log::ui::info(&format!(
            "  {}: 留 {} 个 ({})",
            pr.provider,
            pr.kept.len(),
            pr.kept.join(", ")
        ));
        if !pr.removed.is_empty() {
            crate::log::ui::info(&format!(
                "  {}: 清 {} 个 ({})",
                pr.provider,
                pr.removed.len(),
                pr.removed.join(", ")
            ));
        }
    }
    let verb = if dry_run { "会留" } else { "留" };
    let verb2 = if dry_run { "会清" } else { "清" };
    crate::log::ui::success(&format!(
        "{verb} {total_kept} 个 skill, {verb2} {total_removed} 个过期"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ai::sources::{ModelEntry, ModelSource};

    #[test]
    fn dedupe_keeps_higher_priority() {
        let entries = vec![
            ModelEntry {
                name: "sonnet".to_string(),
                source: ModelSource::BuiltinAlias, // priority 2
            },
            ModelEntry {
                name: "sonnet".to_string(),
                source: ModelSource::ConfigFile(std::path::PathBuf::from("/x")), // priority 0
            },
            ModelEntry {
                name: "opus".to_string(),
                source: ModelSource::BuiltinAlias,
            },
        ];
        let r = dedupe_by_priority(&entries);
        assert_eq!(r.len(), 2);
        // sonnet 留了 ConfigFile 版本
        let sonnet = r.iter().find(|e| e.name == "sonnet").unwrap();
        assert!(matches!(sonnet.source, ModelSource::ConfigFile(_)));
        // opus 唯一 BuiltinAlias 保留
        assert!(r.iter().any(|e| e.name == "opus"));
    }

    #[test]
    fn dedupe_preserves_first_occurrence_order() {
        // 头次出现的顺序决定输出顺序
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
                name: "a".to_string(),
                source: ModelSource::ConfigFile(std::path::PathBuf::from("/x")),
            },
        ];
        let r = dedupe_by_priority(&entries);
        assert_eq!(r[0].name, "a");
        assert_eq!(r[1].name, "b");
    }

    #[test]
    fn dedupe_handles_empty() {
        let r = dedupe_by_priority(&[]);
        assert!(r.is_empty());
    }

    #[test]
    fn compute_stale_dry_returns_unlisted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("frank-ask-claude-sonnet")).unwrap();
        std::fs::create_dir_all(tmp.path().join("frank-ask-claude-old-model")).unwrap();
        let keep = vec!["sonnet".to_string()];
        let stale = compute_stale_skills_dry(tmp.path(), "claude", &keep);
        assert_eq!(stale, vec!["frank-ask-claude-old-model"]);
        // 干跑 — 文件还在
        assert!(tmp.path().join("frank-ask-claude-old-model").exists());
    }

    #[test]
    fn refresh_one_writes_skills_for_known_provider() {
        // 用 dry-run 不实际写到用户 ~/.claude/skills/
        let pr = refresh_one("claude", 3, true, false).unwrap();
        assert_eq!(pr.provider, "claude");
        // claude 至少有 BuiltinAlias 兜底 (sonnet/opus/haiku) 所以非空
        // 但实际 collect_user_models 可能拉到用户的配置 (前 2 路非空就不走兜底)
        // 总之 kept 不该空
        assert!(!pr.kept.is_empty());
    }

    #[test]
    fn refresh_one_caps_at_max() {
        let pr = refresh_one("claude", 1, true, false).unwrap();
        // 顶多 1 个 (--max 1)
        assert!(pr.kept.len() <= 1);
    }

    #[test]
    fn refresh_one_clean_only_skips_writes_but_reports_kept() {
        let pr = refresh_one("claude", 3, true, true).unwrap();
        // clean_only=true: kept 列表是"会保留的清单", 不实际写
        assert!(!pr.kept.is_empty());
    }
}
