//! 读 env vars 临时覆盖 model.
//!
//! 用户跑 `ANTHROPIC_MODEL=opus claude --print "test"` 时是临时切, 各家 CLI 都支持这种
//! env var 覆盖. frank 把这些 env var 也列出来, UI 前缀显示 `[env]` 让用户知道
//! "这是当前 shell 临时设的, 不是写在配置里的".
//!
//! # provider → env var 名映射 (每家约定俗成的官方变量名)
//!
//! - claude → `ANTHROPIC_MODEL`
//! - codex → `OPENAI_MODEL`
//! - gemini → `GEMINI_MODEL`
//! - opencode → (无统一 env var, opencode 是 wrapper 框架, 没硬编码哪家 LLM)

use super::{ModelEntry, ModelSource};

/// provider → env var 名映射. opencode 没有自己的 env var 约定, 跳过.
const ENV_VAR_FOR: &[(&str, &str)] = &[
    ("claude", "ANTHROPIC_MODEL"),
    ("codex", "OPENAI_MODEL"),
    ("gemini", "GEMINI_MODEL"),
];

/// 读某 provider 的 env var, 有值 → 1 条 ModelEntry, 无值 → 空 Vec.
#[must_use]
pub fn read_for(provider: &str) -> Vec<ModelEntry> {
    let Some(var_name) = ENV_VAR_FOR
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, v)| *v)
    else {
        return Vec::new();
    };

    match std::env::var(var_name) {
        Ok(value) if !value.is_empty() => vec![ModelEntry {
            name: value,
            source: ModelSource::EnvVar(var_name.to_string()),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注意: env var 是进程全局共享, 跟 sources/* fs 测试用同一把锁串行.
    // 不真去碰 ANTHROPIC_MODEL 之外, 用 `OPENAI_MODEL` 测试 (frank 本身不读这俩).

    #[test]
    fn returns_empty_when_env_var_unset() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        // claude 对应 ANTHROPIC_MODEL — 临时 remove 测无值情况
        let prev = std::env::var_os("ANTHROPIC_MODEL");
        std::env::remove_var("ANTHROPIC_MODEL");
        let r = read_for("claude");
        if let Some(v) = prev {
            std::env::set_var("ANTHROPIC_MODEL", v);
        }
        assert!(r.is_empty());
    }

    #[test]
    fn returns_entry_when_env_var_set() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let prev = std::env::var_os("ANTHROPIC_MODEL");
        std::env::set_var("ANTHROPIC_MODEL", "opus-test-value");
        let r = read_for("claude");
        // 还原
        if let Some(v) = prev {
            std::env::set_var("ANTHROPIC_MODEL", v);
        } else {
            std::env::remove_var("ANTHROPIC_MODEL");
        }
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "opus-test-value");
        assert!(matches!(r[0].source, ModelSource::EnvVar(ref name) if name == "ANTHROPIC_MODEL"));
    }

    #[test]
    fn returns_empty_for_empty_env_var() {
        let _lock = crate::cli::ai::history_store::test_home_lock();
        let prev = std::env::var_os("OPENAI_MODEL");
        std::env::set_var("OPENAI_MODEL", "");
        let r = read_for("codex");
        if let Some(v) = prev {
            std::env::set_var("OPENAI_MODEL", v);
        } else {
            std::env::remove_var("OPENAI_MODEL");
        }
        assert!(r.is_empty());
    }

    #[test]
    fn returns_empty_for_opencode_no_env_var() {
        // opencode 没映射 — 返回空就完事 (不读 env, 不需要锁)
        let r = read_for("opencode");
        assert!(r.is_empty());
    }

    #[test]
    fn returns_empty_for_unknown_provider() {
        let r = read_for("unknown-fake");
        assert!(r.is_empty());
    }
}
