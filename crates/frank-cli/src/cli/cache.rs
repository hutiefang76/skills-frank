//! `frank cache <sub>` — `~/.frank/cache/` 可视化 + 清理 (v0.9-1)。
//!
//! 解决用户视角痛点: 之前只能 `ls ~/.frank/cache/` 看到 sha hash 不知道哪个是哪个 skill.
//! 现在 `frank cache list` 列 (name, url, size, age), `clear` 单清/全清.

use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};

use crate::installer::git;
use crate::state::State;

/// `frank cache` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令: list / clear。
    #[command(subcommand)]
    pub command: CacheCommand,
}

/// cache 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum CacheCommand {
    /// 列出 ~/.frank/cache/ 所有 cached repo, 关联 state.json 找出 url + skill name。
    List,

    /// 清空 cache (不删 state.json; 下次 install/update 自动重 clone)。
    Clear {
        /// 只清这个 skill 的 cache (不传 = 清全部)。
        name: Option<String>,

        /// 跳过 'y/n' 交互确认。
        #[arg(long)]
        yes: bool,
    },
}

/// 单个 cache entry 的元数据 (给 list 显示用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    /// cache 目录名 (16 hex sha256(url) 前缀)。
    pub key: String,
    /// 反查到的 skill name (state 没记录时 None = orphan)。
    pub name: Option<String>,
    /// 反查到的 url (orphan 时 None)。
    pub url: Option<String>,
    /// 目录总字节大小 (递归)。
    pub size_bytes: u64,
}

/// 列出 cache_root 下所有 entry, 跟 state 对照填 name/url。
///
/// `cache_root` 用 [`git::cache_root`] (生产) 或测试用 tmp dir。
pub fn collect_entries(cache_root: &Path, state: &State) -> Result<Vec<CacheEntry>> {
    if !cache_root.exists() {
        return Ok(Vec::new());
    }
    // 建 cache_key → (name, url) map (从 state 反查)
    let mut by_key: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for s in state.iter() {
        // source_path 是 cache/<key>/<subpath?>, 取第一段
        if let Some(key) = s
            .source_path
            .components()
            .skip_while(|c| c.as_os_str() != "cache")
            .nth(1)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
        {
            // 我们没存 url 直接 in state, 只能用 source_ref/source_path 反推. 这里
            // url 取不到的话用 source_ref(commit sha) 作为占位.
            by_key.insert(key, (s.name.clone(), format!("sha:{}", s.source_ref)));
        }
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(cache_root).with_context(|| format!("read_dir {}", cache_root.display()))?
    {
        let entry = entry?;
        let key = entry.file_name().to_string_lossy().into_owned();
        if key.starts_with('.') {
            continue;
        }
        let size_bytes = dir_size(&entry.path()).unwrap_or(0);
        let (name, url) = by_key
            .get(&key)
            .map_or((None, None), |(n, u)| (Some(n.clone()), Some(u.clone())));
        out.push(CacheEntry {
            key,
            name,
            url,
            size_bytes,
        });
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.size_bytes));
    Ok(out)
}

/// 递归算目录字节数 (浅:不跟 symlink 不展开)。
fn dir_size(path: &std::path::Path) -> Result<u64> {
    let mut total = 0u64;
    let stack = std::collections::VecDeque::from([path.to_path_buf()]);
    let mut stack = stack;
    while let Some(p) = stack.pop_front() {
        let md = fs::symlink_metadata(&p)?;
        if md.is_dir() {
            for e in fs::read_dir(&p)? {
                stack.push_back(e?.path());
            }
        } else {
            total += md.len();
        }
    }
    Ok(total)
}

/// 删指定 cache_root 下名为 `key` 的子目录。返回 (existed, removed_size).
pub fn clear_one(cache_root: &Path, key: &str) -> Result<(bool, u64)> {
    let p = cache_root.join(key);
    if !p.exists() {
        return Ok((false, 0));
    }
    let size = dir_size(&p).unwrap_or(0);
    fs::remove_dir_all(&p).with_context(|| format!("rm {}", p.display()))?;
    Ok((true, size))
}

/// 删 cache_root 下所有子目录。返回 (count, total_bytes).
pub fn clear_all(cache_root: &Path) -> Result<(usize, u64)> {
    if !cache_root.exists() {
        return Ok((0, 0));
    }
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in fs::read_dir(cache_root)? {
        let p = entry?.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| !n.starts_with('.'))
        {
            let size = dir_size(&p).unwrap_or(0);
            fs::remove_dir_all(&p)?;
            count += 1;
            total += size;
        }
    }
    Ok((count, total))
}

/// 人类可读字节数: 1234 → "1.2 KB"。
#[allow(clippy::cast_precision_loss)] // 容量信息可读性 > 1 byte 精度
fn fmt_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

/// 执行 cache 命令 (CLI 入口)。
pub fn run(args: Args) -> Result<()> {
    let cache_root = git::cache_root()?;
    let state = State::load_default()?;
    match args.command {
        CacheCommand::List => cmd_list(&cache_root, &state),
        CacheCommand::Clear { name, yes } => cmd_clear(&cache_root, &state, name.as_deref(), yes),
    }
}

fn cmd_list(cache_root: &Path, state: &State) -> Result<()> {
    let entries = collect_entries(cache_root, state)?;
    if entries.is_empty() {
        crate::log::ui::info(&format!(
            "no cache entries in {} (run `frank install` to populate)",
            cache_root.display()
        ));
        return Ok(());
    }
    let total: u64 = entries.iter().map(|e| e.size_bytes).sum();
    crate::log::ui::section(&format!(
        "{} cache {} entries, total {}",
        cache_root.display(),
        entries.len(),
        fmt_size(total)
    ));
    println!("  {:<16}  {:<28}  {:>10}  URL/REF", "KEY", "SKILL", "SIZE");
    println!("  {}", "-".repeat(80));
    for e in &entries {
        let name = e.name.as_deref().unwrap_or("(orphan)");
        let url = e.url.as_deref().unwrap_or("-");
        println!(
            "  {:<16}  {:<28}  {:>10}  {}",
            e.key,
            truncate(name, 28),
            fmt_size(e.size_bytes),
            truncate(url, 40)
        );
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let take: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{take}…")
    }
}

fn cmd_clear(cache_root: &Path, state: &State, name: Option<&str>, yes: bool) -> Result<()> {
    if let Some(n) = name {
        // 单清: 用 name 反查到 source_path → 取 cache key
        let s = state
            .get(n)
            .ok_or_else(|| anyhow!("`{n}` not in state.json; check `frank list`"))?;
        let key = s
            .source_path
            .components()
            .skip_while(|c| c.as_os_str() != "cache")
            .nth(1)
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("can't parse cache key from {}", s.source_path.display()))?;
        if !yes {
            crate::log::ui::warn(&format!(
                "about to clear cache for `{n}` (key={key}). re-run with --yes."
            ));
            return Ok(());
        }
        let (existed, size) = clear_one(cache_root, &key)?;
        if existed {
            crate::log::ui::success(&format!("cleared `{n}` cache ({} freed)", fmt_size(size)));
        } else {
            crate::log::ui::info(&format!("`{n}` cache already empty"));
        }
        return Ok(());
    }
    // 全清
    if !yes {
        let entries = collect_entries(cache_root, state)?;
        let total: u64 = entries.iter().map(|e| e.size_bytes).sum();
        crate::log::ui::warn(&format!(
            "about to clear {} cache entries ({}). re-run with --yes.",
            entries.len(),
            fmt_size(total)
        ));
        return Ok(());
    }
    let (count, total) = clear_all(cache_root)?;
    crate::log::ui::success(&format!(
        "cleared {count} cache entries ({} freed)",
        fmt_size(total)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::schema::Platform;
    use crate::state::SkillState;
    use chrono::Utc;

    fn mk_state_with_one(name: &str, cache_key: &str) -> State {
        let tf = tempfile::NamedTempFile::new().unwrap();
        let mut s = State::load(tf.path().to_path_buf()).unwrap();
        s.put(SkillState {
            name: name.into(),
            source_ref: "abc1234".into(),
            source_path: PathBuf::from(format!("/cache/{cache_key}/repo")),
            platforms: vec![Platform::Claude],
            installed_at: Utc::now(),
            enabled: true,
            visibility: None,
        });
        s
    }

    fn touch_cache_dir(root: &Path, key: &str, content_size: usize) -> PathBuf {
        let p = root.join(key);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("payload.bin"), vec![0u8; content_size]).unwrap();
        p
    }

    #[test]
    fn collect_entries_empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = mk_state_with_one("doris-ops", "abc1234567890abc");
        let entries = collect_entries(tmp.path(), &state).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn collect_entries_matches_name_from_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        touch_cache_dir(&root, "abc1234567890abc", 1024);
        let state = mk_state_with_one("doris-ops", "abc1234567890abc");
        let entries = collect_entries(&root, &state).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name.as_deref(), Some("doris-ops"));
        assert_eq!(entries[0].size_bytes, 1024);
    }

    #[test]
    fn collect_entries_marks_orphan_when_state_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        touch_cache_dir(&root, "orphan-key-xyz1", 512);
        let tf = tempfile::NamedTempFile::new().unwrap();
        let state = State::load(tf.path().to_path_buf()).unwrap(); // 空 state
        let entries = collect_entries(&root, &state).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].name.is_none());
        assert_eq!(entries[0].size_bytes, 512);
    }

    #[test]
    fn clear_one_existing_returns_size() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        touch_cache_dir(&root, "deadbeef", 2048);
        let (existed, size) = clear_one(&root, "deadbeef").unwrap();
        assert!(existed);
        assert_eq!(size, 2048);
        assert!(!root.join("deadbeef").exists());
    }

    #[test]
    fn clear_one_missing_returns_existed_false() {
        let tmp = tempfile::tempdir().unwrap();
        let (existed, size) = clear_one(tmp.path(), "nonexist").unwrap();
        assert!(!existed);
        assert_eq!(size, 0);
    }

    #[test]
    fn clear_all_removes_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        touch_cache_dir(&root, "k1", 100);
        touch_cache_dir(&root, "k2", 200);
        touch_cache_dir(&root, "k3", 300);
        let (count, total) = clear_all(&root).unwrap();
        assert_eq!(count, 3);
        assert_eq!(total, 600);
        // root 还在, 但是空
        let remaining = fs::read_dir(&root).unwrap().count();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn fmt_size_human_readable() {
        assert_eq!(fmt_size(500), "500 B");
        assert_eq!(fmt_size(1500), "1.5 KB");
        assert_eq!(fmt_size(2_500_000), "2.4 MB");
    }
}
