//! v0.10.8 D4: 按用户配的 model 自动生成 slash command skill。
//!
//! # 干啥
//!
//! 用户在 cc-switch 配了 5 个 claude 模型 (官方 sonnet / opus, zkeys-免费, kimi-k2.5, ...),
//! `frank refresh-skills` 就给每个生成一个 skill 目录:
//!
//! ```text
//! ~/.claude/skills/frank-ask-claude-sonnet/SKILL.md
//! ~/.claude/skills/frank-ask-claude-opus/SKILL.md
//! ~/.claude/skills/frank-ask-claude-kimi-k2-5/SKILL.md
//! ...
//! ```
//!
//! 用户在 claude session 里输入 `/frank-ask-claude-kimi-k2-5 你好` 就触发该 skill,
//! 跑 `frank ai ask --to claude --model kimi-k2.5 "你好"`. **比写死的 5 个 slash
//! command 灵活得多** — 用户换模型不用改 frank 代码。
//!
//! # 名字转换
//!
//! slash command 不能有点 `.` 和斜杠 `/`. `kimi-k2.5` → `kimi-k2-5`,
//! `claude/sonnet` → `claude-sonnet`. 简单字符替换, 不丢信息。
//!
//! # 防累积
//!
//! 用户从 cc-switch 删一个 provider 再 `frank refresh-skills`, 旧 skill 目录
//! 也要清掉 — 否则 slash command 累积越来越多, 列表乱。`clean_stale_skills` 干这事。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 单个 skill 的生成模板 (一个 provider × 一个 model)。
#[derive(Debug, Clone)]
pub struct SkillTemplate {
    /// provider 名 (例 `claude` / `codex` / `gemini` / `opencode`)。
    pub provider: String,
    /// 原始 model 名 (例 `kimi-k2.5`, 含点), **不**做转换。
    pub model: String,
    /// 目标 skill 目录的父目录 (例 `~/.claude/skills/`)。
    pub target_dir: PathBuf,
}

impl SkillTemplate {
    /// skill 名 (例 `frank-ask-claude-kimi-k2-5`)。
    ///
    /// 跟 slash command 一致 — 用户输入 `/<skill_name> <prompt>` 触发。
    ///
    /// v0.15: model 名已含 provider 前缀时去重 — models.dev 的 id 常带前缀
    /// (`claude-opus-4-5`, `gemini-3-pro`), 避免双前缀 `frank-ask-claude-claude-opus-4-5`
    /// → 简洁的 `frank-ask-claude-opus-4-5`. codex 模型 (`gpt-5.5`) 无 `codex-` 前缀, 不受影响.
    #[must_use]
    pub fn skill_name(&self) -> String {
        skill_name_for(&self.provider, &self.model)
    }

    /// 完整目标路径 (例 `~/.claude/skills/frank-ask-claude-kimi-k2-5/`)。
    #[must_use]
    pub fn skill_dir(&self) -> PathBuf {
        self.target_dir.join(self.skill_name())
    }

    /// SKILL.md 完整路径。
    #[must_use]
    pub fn skill_md(&self) -> PathBuf {
        self.skill_dir().join("SKILL.md")
    }

    /// 渲染 SKILL.md 文本内容。
    ///
    /// frontmatter (YAML) + 一段大白话 description + 触发命令模板。
    #[must_use]
    pub fn render(&self) -> String {
        let skill_name = self.skill_name();
        let model = &self.model;
        let provider = &self.provider;
        let version = env!("CARGO_PKG_VERSION");
        format!(
            r#"---
name: {skill_name}
description: '用 {provider} 的 {model} 模型回答。'
---

# {skill_name}

用户**明确要 {provider} 的 `{model}` 模型**时, 用 Bash 工具调:

```bash
frank ai ask --to {provider} --from claude --source-cwd "$PWD" --model {model} "<用户的 prompt 原话>"
```

**约定**:
- 不修改 prompt (不翻译/不优化/不加 system prompt)
- 模型固定 `{model}` (本 skill 名字就带它)
- stdout 原样返回, 不加 "X 说:" 这种修饰
- 默认 `--timeout 300`

## 自动生成

本 skill 是 frank v{version} 用 `frank refresh-skills` 自动生成的, 来源:
你的 cc-switch / CLI 配置 / 环境变量里有 `{provider}` + `{model}`.

**别手动改这个文件** — 下次 `frank refresh-skills` 会覆盖. 想加自己 skill 就放别的目录。
"#
        )
    }
}

/// 把 model 名转成 slash command 安全字符串。
///
/// slash command 只认 `[a-zA-Z0-9_-]`, 点 `.` 斜杠 `/` 都不行 → 转 dash。
/// 同时小写化避免 case 敏感差异。
///
/// 例:
/// - `kimi-k2.5` → `kimi-k2-5`
/// - `claude/sonnet` → `claude-sonnet`
/// - `GPT-5.4-Mini` → `gpt-5-4-mini`
/// - `sonnet[1m]` → `sonnet-1m-` (方括号也转 dash, 末尾 dash 不动)
#[must_use]
pub fn safe_model_name(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// 单一权威的 skill 名构造 (provider + model → `frank-ask-<provider>-<safe-model>`).
///
/// v0.15: model 已含 provider 前缀时去重 — models.dev 的 id 常带前缀 (`claude-opus-4-5`,
/// `gemini-3-pro`), 避免双前缀 `frank-ask-claude-claude-opus-4-5` → `frank-ask-claude-opus-4-5`.
/// codex (`gpt-5.5`) 无 `codex-` 前缀不受影响; `gemma-*` 不含 `gemini-` 前缀也保留.
///
/// `SkillTemplate::skill_name` 和 `clean_stale_skills` 都走这个, 保证生成名 == 清理保留名,
/// 不会因逻辑分叉导致每次 refresh 误删刚生成的 skill (churn).
#[must_use]
pub fn skill_name_for(provider: &str, model: &str) -> String {
    let safe = safe_model_name(model);
    let dup_prefix = format!("{provider}-");
    let model_part = safe.strip_prefix(&dup_prefix).unwrap_or(&safe);
    format!("frank-ask-{provider}-{model_part}")
}

/// 原子写: 先写 tmp 再 rename, 防止 SIGKILL 时写半截污染目录。
///
/// 已存在内容**一样**时直接跳过 (不动文件 mtime, 减少不必要 IO).
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    // 已存在且内容一样 → 跳过
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    let parent = path
        .parent()
        .with_context(|| format!("path {} has no parent dir", path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("frank-skill")
    ));
    fs::write(&tmp, content).with_context(|| format!("write tmp {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// 把一个 skill 写到磁盘 (含目录创建 + SKILL.md 原子写)。
///
/// 已存在内容一样 → 不动文件 (write_atomic 早 return).
pub fn write_skill(template: &SkillTemplate) -> Result<()> {
    let content = template.render();
    let path = template.skill_md();
    write_atomic(&path, &content)
}

/// 删除该 provider 名下"frank- 前缀 + 不在当前 model 列表内"的过期 skill 目录。
///
/// 例: 用户从 cc-switch 删了 `kimi-k2.5`, 当前 current_models = `[sonnet, opus]`,
/// 那 `frank-ask-claude-kimi-k2-5/` 就会被清掉, `frank-ask-claude-sonnet/` 保留。
///
/// **只动 frank- 前缀的目录** — 用户自己装的 skill (任何前缀) 都不动。
///
/// 返回: 实际删掉的 skill 名列表 (用于 UI 打印).
pub fn clean_stale_skills(
    target_dir: &Path,
    provider: &str,
    current_models: &[String],
) -> Result<Vec<String>> {
    // 当前应该保留的 skill 名 set
    let keep: HashSet<String> = current_models
        .iter()
        .map(|m| skill_name_for(provider, m))
        .collect();
    let prefix = format!("frank-ask-{provider}-");

    let mut removed = Vec::new();
    if !target_dir.exists() {
        return Ok(removed); // 目录不存在 → 啥都不用清
    }
    let entries =
        fs::read_dir(target_dir).with_context(|| format!("read_dir {}", target_dir.display()))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // 只看目录 (跳过符号链接和文件 — 用户的 link 不动)
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        // 只动我们前缀的, 不在 keep set 里
        if name.starts_with(&prefix) && !keep.contains(&name) {
            let p = entry.path();
            if fs::remove_dir_all(&p).is_ok() {
                removed.push(name);
            }
        }
    }
    Ok(removed)
}

/// 拿目标平台的 skills 目录 (例 claude → `~/.claude/skills/`).
///
/// 跟 `crate::adapter::for_platform(p).platform_dir()` 一致 — 但为避免引入 adapter
/// trait 在这浅薄场景, 直接写死路径 (3 家都是固定路径).
///
/// 不存在 → 返回 None (调用方跳过该 provider 即可).
#[must_use]
pub fn skills_dir_for(provider: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = match provider {
        // claude/codex 都把 model variant 写到 claude 平台 (slash command 在 claude session 触发);
        // codex 用户也常在 claude 里调 (`/frank-ask-codex-gpt-5.5`).
        // 所以**所有 provider 的 skill 都写到 ~/.claude/skills/** — 跟现有 5 个固定 skill 一致.
        "claude" | "codex" | "gemini" | "opencode" => home.join(".claude").join("skills"),
        _ => return None,
    };
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_model_name_handles_dots() {
        assert_eq!(safe_model_name("kimi-k2.5"), "kimi-k2-5");
        assert_eq!(safe_model_name("gpt-5.4-mini"), "gpt-5-4-mini");
    }

    #[test]
    fn safe_model_name_handles_slashes() {
        // 简单 ASCII 斜杠 → dash
        assert_eq!(safe_model_name("openai/gpt-4"), "openai-gpt-4");
        // 中文字符也转 dash (2 个汉字 + 1 斜杠 → 3 个 dash)
        assert_eq!(safe_model_name("zkeys-免费/sonnet"), "zkeys----sonnet");
    }

    #[test]
    fn safe_model_name_lowercases() {
        assert_eq!(safe_model_name("GPT-5.4-Mini"), "gpt-5-4-mini");
    }

    #[test]
    fn safe_model_name_handles_brackets() {
        assert_eq!(safe_model_name("sonnet[1m]"), "sonnet-1m-");
    }

    #[test]
    fn skill_name_combines_provider_and_safe_model() {
        let tpl = SkillTemplate {
            provider: "claude".to_string(),
            model: "kimi-k2.5".to_string(),
            target_dir: PathBuf::from("/tmp/skills"),
        };
        assert_eq!(tpl.skill_name(), "frank-ask-claude-kimi-k2-5");
    }

    #[test]
    fn skill_name_strips_dup_provider_prefix() {
        // v0.15: models.dev id 带 provider 前缀 → 去重避免双前缀
        assert_eq!(
            skill_name_for("claude", "claude-opus-4-5"),
            "frank-ask-claude-opus-4-5"
        );
        assert_eq!(
            skill_name_for("gemini", "gemini-3-pro"),
            "frank-ask-gemini-3-pro"
        );
        // 不带前缀的不动 (codex gpt-5.5)
        assert_eq!(skill_name_for("codex", "gpt-5.5"), "frank-ask-codex-gpt-5-5");
        // gemma 不带 gemini 前缀, 保留
        assert_eq!(
            skill_name_for("gemini", "gemma-4-31b-it"),
            "frank-ask-gemini-gemma-4-31b-it"
        );
    }

    #[test]
    fn clean_stale_keep_names_match_generated() {
        // 关键回归: clean_stale_skills 的 keep set 必须 == skill_name() 生成名,
        // 否则每次 refresh 误删刚生成的 (churn). 两边都走 skill_name_for 保证一致.
        let model = "claude-opus-4-5".to_string();
        let generated = SkillTemplate {
            provider: "claude".to_string(),
            model: model.clone(),
            target_dir: PathBuf::from("/tmp"),
        }
        .skill_name();
        assert_eq!(generated, skill_name_for("claude", &model));
    }

    #[test]
    fn render_includes_correct_model_and_provider() {
        let tpl = SkillTemplate {
            provider: "claude".to_string(),
            model: "haiku".to_string(),
            target_dir: PathBuf::from("/tmp"),
        };
        let s = tpl.render();
        // YAML frontmatter 含 skill name
        assert!(s.contains("name: frank-ask-claude-haiku"));
        // 触发命令含正确 provider + model
        assert!(s.contains("--to claude"));
        assert!(s.contains("--model haiku"));
        // v0.10.9: 简化版 description (旧版字太多 UI 截断)
        assert!(s.contains("用 claude 的 haiku 模型回答"));
    }

    #[test]
    fn render_has_yaml_frontmatter() {
        let tpl = SkillTemplate {
            provider: "codex".to_string(),
            model: "gpt-5.5".to_string(),
            target_dir: PathBuf::from("/tmp"),
        };
        let s = tpl.render();
        // 开头是 YAML frontmatter
        assert!(s.starts_with("---\n"));
        assert!(s.contains("\n---\n"));
    }

    #[test]
    fn write_atomic_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("file.md");
        write_atomic(&path, "hello").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_atomic_skips_when_content_same() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.md");
        write_atomic(&path, "v1").unwrap();
        let mtime_before = fs::metadata(&path).unwrap().modified().unwrap();
        // 二次写 same content
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_atomic(&path, "v1").unwrap();
        let mtime_after = fs::metadata(&path).unwrap().modified().unwrap();
        // mtime 应该不变 (短路了 write)
        assert_eq!(mtime_before, mtime_after);
    }

    #[test]
    fn write_atomic_replaces_when_content_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("file.md");
        write_atomic(&path, "v1").unwrap();
        write_atomic(&path, "v2").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }

    #[test]
    fn write_skill_creates_skill_md_in_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let tpl = SkillTemplate {
            provider: "claude".to_string(),
            model: "sonnet".to_string(),
            target_dir: tmp.path().to_path_buf(),
        };
        write_skill(&tpl).unwrap();
        let md = tmp.path().join("frank-ask-claude-sonnet").join("SKILL.md");
        assert!(md.exists());
        let content = fs::read_to_string(&md).unwrap();
        assert!(content.contains("frank ai ask --to claude"));
        assert!(content.contains("--model sonnet"));
    }

    #[test]
    fn clean_stale_removes_unlisted_frank_skills() {
        let tmp = tempfile::tempdir().unwrap();
        // 装 3 个 skill 进去, 其中 2 个用 frank- 前缀
        fs::create_dir_all(tmp.path().join("frank-ask-claude-sonnet")).unwrap();
        fs::create_dir_all(tmp.path().join("frank-ask-claude-kimi-k2-5")).unwrap();
        fs::create_dir_all(tmp.path().join("my-own-skill")).unwrap();
        // 当前只留 sonnet
        let current = vec!["sonnet".to_string()];
        let removed = clean_stale_skills(tmp.path(), "claude", &current).unwrap();
        assert_eq!(removed, vec!["frank-ask-claude-kimi-k2-5"]);
        // sonnet 留着, 用户自己 skill 也留着
        assert!(tmp.path().join("frank-ask-claude-sonnet").exists());
        assert!(tmp.path().join("my-own-skill").exists());
        // kimi-k2-5 被清
        assert!(!tmp.path().join("frank-ask-claude-kimi-k2-5").exists());
    }

    #[test]
    fn clean_stale_skips_other_provider_skills() {
        let tmp = tempfile::tempdir().unwrap();
        // 装 codex 的 skill, 但调 clean 时指定 provider=claude
        fs::create_dir_all(tmp.path().join("frank-ask-codex-gpt-5-5")).unwrap();
        let removed = clean_stale_skills(tmp.path(), "claude", &[]).unwrap();
        assert!(removed.is_empty());
        // codex 的不动
        assert!(tmp.path().join("frank-ask-codex-gpt-5-5").exists());
    }

    #[test]
    fn clean_stale_handles_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let no_dir = tmp.path().join("nope");
        let removed = clean_stale_skills(&no_dir, "claude", &[]).unwrap();
        assert!(removed.is_empty()); // 目录不存在不报错
    }

    #[test]
    fn skills_dir_for_known_providers() {
        for p in ["claude", "codex", "gemini", "opencode"] {
            let dir = skills_dir_for(p).expect("known provider");
            assert!(dir.ends_with(".claude/skills"), "got {dir:?}");
        }
    }

    #[test]
    fn skills_dir_for_unknown_returns_none() {
        assert!(skills_dir_for("foobar").is_none());
    }
}
