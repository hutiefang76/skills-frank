//! AI 一问一答历史的持久化层。
//!
//! # 为什么拆出来 (v0.10.7 D4)
//!
//! 老版本只把摘要 (200 字符) 写进一个 JSONL 文件 (`~/.frank/ai_history.jsonl`),
//! 用户**没法**回看完整 prompt + 完整回答 — 历史只是个"列表", 看不到全文。
//!
//! 新版本拆成两份:
//!
//! - **索引文件** `~/.frank/ai_history.jsonl` — 一行一条 JSON, 只存摘要 +
//!   一个**短码 id**。列表的时候只读这文件, 内存吃得起 (10 万条 ≈ 30 MB)。
//! - **全文文件** `~/.frank/ai-history-full/<id>.json` — 一文件一条完整记录
//!   (完整 prompt + 完整 reply + 时间戳)。要看 / 删才打开。
//!
//! 这样 list 不卡, 删 (一个文件) 也快, 量大也不撑爆。
//!
//! # id 长啥样
//!
//! `YYYYMMDD-HHMMSS-XXXX`, 例 `20260525-143022-a7f2`。
//! 前面是 UTC 时间 (人能直接看出什么时候发的), 后 4 位 hex 随机
//! (同一秒并发的不撞)。
//!
//! # 向后兼容
//!
//! 老数据 (没 id 字段) 也能读。读的时候发现没 id 就**临时**按时间戳生成一个,
//! 但**不**回写文件 (老条目就没全文, 用户只能看摘要)。

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use fs2::FileExt;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// 防多窗口并发写损坏的锁: 拿不到就重试, 5s 还拿不到就 bail.
///
/// 用法: `wait_for_lock(&f, "ai_history.jsonl")?;` — 失败时 anyhow 错误带文件名,
/// 用户能直接看到哪个文件被卡 (理论上 5s 拿不到锁意味着另一个进程 hang 了).
///
/// 为什么不直接用 `lock_exclusive` 阻塞? 因为阻塞死等用户没法 Ctrl-C,
/// 5s 已经是宽松上限 (正常拿锁 <1ms).
fn wait_for_lock(f: &File, what: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        match f.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(_) if start.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                anyhow::bail!("{what}: 5s 内拿不到文件锁, 是不是有别的 frank 卡住了? ({e})")
            }
        }
    }
}

/// 一条 history 的短码 (例 `20260525-143022-a7f2`)。
///
/// 包成 `String` 一层而非裸字符串, 是为了让函数签名更清楚 + 未来加格式校验。
pub type HistoryId = String;

/// 索引一行 (摘要)。
///
/// 写进 `~/.frank/ai_history.jsonl`, 一行一条。
/// `id` 是 v0.10.7 新加的字段, 老条目无 — 读的时候补一个临时 id。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    /// 短码 id (新条目必有, 老条目读时补)。
    #[serde(default)]
    pub id: HistoryId,
    /// ISO-8601 UTC 时间戳。
    pub ts: String,
    /// 调用方 provider (claude / codex / 等)。
    pub from: String,
    /// 目标 provider。
    pub to: String,
    /// 调用方工作目录 (可选)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_cwd: Option<String>,
    /// 用户自定义 tag (可选)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_tag: Option<String>,
    /// 用了哪个 model (`--model` 传的, 可选)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// prompt 前 200 字符 (展示用, 全文在 `<id>.json`)。
    pub prompt_excerpt: String,
    /// 回答前 200 字符 (展示用, 全文在 `<id>.json`)。
    pub response_excerpt: String,
    /// 状态 `"ok"` / `"err"`。
    pub status: String,
    /// 出错信息 (`status="err"` 时有)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// 耗时 (毫秒)。
    pub latency_ms: u64,
}

/// 全文文件 (`~/.frank/ai-history-full/<id>.json`) 的内容。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FullRecord {
    /// 完整 prompt (用户原话, 不截)。
    pub prompt: String,
    /// 完整回答 (CLI 原文, 不截)。
    pub response: String,
    /// 时间戳 (跟索引行一致, 方便单独 ls 看时间)。
    pub ts: String,
}

/// list 时的过滤条件。
#[derive(Default, Debug)]
pub struct ListFilter {
    /// 只看某个目标 provider (`--to claude` 等)。
    pub provider: Option<String>,
    /// 只看某个状态 (`ok` / `err`)。
    pub status: Option<String>,
    /// 只看某个时间之后 (ISO-8601 或 `YYYY-MM-DD`)。
    pub since: Option<DateTime<Utc>>,
    /// 只看某个调用方 cwd (子串包含)。
    pub cwd: Option<String>,
    /// 取前几条 (`None` = 全部)。
    pub limit: Option<usize>,
}

/// 持久化层入口。
///
/// 无状态结构 (只是放方法的容器), 所有操作都是直接读 / 写文件。
pub struct HistoryStore;

impl HistoryStore {
    /// 索引文件路径 (`~/.frank/ai_history.jsonl`)。
    pub fn index_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".frank").join("ai_history.jsonl"))
    }

    /// 全文文件夹路径 (`~/.frank/ai-history-full/`)。
    pub fn full_dir() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".frank").join("ai-history-full"))
    }

    /// 全文文件路径 (`~/.frank/ai-history-full/<id>.json`)。
    pub fn full_path(id: &str) -> Option<PathBuf> {
        Self::full_dir().map(|d| d.join(format!("{id}.json")))
    }

    /// 生成一个新 id (基于当前 UTC 时间 + 4 位随机 hex)。
    ///
    /// 例: `20260525-143022-a7f2`
    #[must_use]
    pub fn new_id() -> HistoryId {
        let now = Utc::now();
        let stamp = now.format("%Y%m%d-%H%M%S");
        let suffix: u16 = rand::thread_rng().gen();
        format!("{stamp}-{suffix:04x}")
    }

    /// 写一条新历史: 索引一行 + 全文一文件。
    ///
    /// 索引文件加锁 (跨平台 advisory lock), 多窗口同时跑 `frank ai ask` 也不串行。
    /// 全文文件**先写 tmp 再 rename** (原子, 不会半写)。
    pub fn append(entry: &HistoryEntry, full_prompt: &str, full_response: &str) -> Result<()> {
        let Some(index_path) = Self::index_path() else {
            return Ok(()); // 没 home dir, 静默跳过
        };
        let Some(full_dir) = Self::full_dir() else {
            return Ok(());
        };
        let Some(full_path) = Self::full_path(&entry.id) else {
            return Ok(());
        };
        if let Some(parent) = index_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::create_dir_all(&full_dir).ok();

        // 1) 全文文件: 先写 .tmp 再 rename (避免半写)
        let full = FullRecord {
            prompt: full_prompt.to_string(),
            response: full_response.to_string(),
            ts: entry.ts.clone(),
        };
        let tmp = full_path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(&full).context("serialize full record")?,
        )
        .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &full_path)
            .with_context(|| format!("rename {} → {}", tmp.display(), full_path.display()))?;

        // 2) 索引行: append 模式 + 防多窗口并发写损坏的锁 (跨平台)
        let line = serde_json::to_string(entry).context("serialize history entry")?;
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .with_context(|| format!("open {}", index_path.display()))?;
        wait_for_lock(&f, &format!("{}", index_path.display()))?;
        // 拿到锁后再写, 用 closure 保证写完一定走 unlock 路径
        let res = (|| -> Result<()> {
            let mut f = f;
            writeln!(f, "{line}").context("write history line")?;
            f.sync_all().context("sync history file")?;
            Ok(())
        })();
        // f 被 closure 拿走, fd close 时 OS 自动放锁
        res
    }

    /// 读索引文件, 按 filter 过滤, 最新在前。
    ///
    /// 老条目 (无 id) 会在内存里补一个临时 id (基于 ts, 用户能继续用 show 但全文文件不存在会提示)。
    pub fn list(filter: &ListFilter) -> Result<Vec<HistoryEntry>> {
        let Some(path) = Self::index_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let f = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut entries: Vec<HistoryEntry> = BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<HistoryEntry>(&l).ok())
            .map(|mut e| {
                // 老条目无 id, 补一个临时的 (从 ts 派生, 没全文文件)
                if e.id.is_empty() {
                    e.id = legacy_id_from_ts(&e.ts);
                }
                e
            })
            .collect();
        entries.reverse(); // 最新在前

        if let Some(p) = filter.provider.as_deref() {
            entries.retain(|e| e.to == p);
        }
        if let Some(s) = filter.status.as_deref() {
            entries.retain(|e| e.status == s);
        }
        if let Some(since) = filter.since {
            entries.retain(|e| {
                // parse 失败的旧条目当 since 之后看 (不丢)
                e.ts.parse::<DateTime<Utc>>().is_ok_and(|t| t >= since)
                    || e.ts.parse::<DateTime<Utc>>().is_err()
            });
        }
        if let Some(c) = filter.cwd.as_deref() {
            entries.retain(|e| e.source_cwd.as_deref().is_some_and(|s| s.contains(c)));
        }
        if let Some(limit) = filter.limit {
            entries.truncate(limit);
        }
        Ok(entries)
    }

    /// 看一条的全文 (`~/.frank/ai-history-full/<id>.json`)。
    ///
    /// 老条目 (临时 id) 找不到文件 → 返回明确错误。
    pub fn show(id: &str) -> Result<FullRecord> {
        let Some(path) = Self::full_path(id) else {
            anyhow::bail!("找不到 home dir");
        };
        if !path.exists() {
            anyhow::bail!(
                "找不到全文文件 {} (老条目可能没全文, 只能在 list 看摘要)",
                path.display()
            );
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// 单删一条: 索引文件去掉对应行 + 删全文文件。
    ///
    /// 索引文件改成 tmp+rename (原子), 加锁防多窗口冲突。
    pub fn delete(id: &str) -> Result<()> {
        let Some(index_path) = Self::index_path() else {
            return Ok(());
        };
        if !index_path.exists() {
            anyhow::bail!("没有 history 文件 ({})", index_path.display());
        }
        let lock_f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .with_context(|| format!("open {} for lock", index_path.display()))?;
        lock_f
            .lock_exclusive()
            .with_context(|| format!("lock {}", index_path.display()))?;

        // 读旧 → 过滤掉 id 那行 → 写 tmp → rename
        let text = std::fs::read_to_string(&index_path)
            .with_context(|| format!("read {}", index_path.display()))?;
        let mut found = false;
        let mut kept: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // 解析一行, 看 id 对不对
            let matches = serde_json::from_str::<HistoryEntry>(line).is_ok_and(|e| {
                let entry_id = if e.id.is_empty() {
                    legacy_id_from_ts(&e.ts)
                } else {
                    e.id
                };
                entry_id == id
            });
            if matches {
                found = true;
            } else {
                kept.push(line.to_string());
            }
        }
        if !found {
            anyhow::bail!("没找到 id `{id}`");
        }

        let tmp = index_path.with_extension("jsonl.tmp");
        let mut tmp_f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        for line in &kept {
            writeln!(tmp_f, "{line}").context("write tmp")?;
        }
        tmp_f.sync_all().context("sync tmp")?;
        drop(tmp_f);
        std::fs::rename(&tmp, &index_path)
            .with_context(|| format!("rename {} → {}", tmp.display(), index_path.display()))?;

        // 删全文 (找不到也 ok, 老条目没文件)
        if let Some(full_path) = Self::full_path(id) {
            std::fs::remove_file(&full_path).ok();
        }
        Ok(())
    }

    /// 批删: 时间戳之前的全删。返回删了多少条。
    pub fn delete_before(cutoff: DateTime<Utc>) -> Result<usize> {
        let Some(index_path) = Self::index_path() else {
            return Ok(0);
        };
        if !index_path.exists() {
            return Ok(0);
        }
        let lock_f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .with_context(|| format!("open {} for lock", index_path.display()))?;
        lock_f
            .lock_exclusive()
            .with_context(|| format!("lock {}", index_path.display()))?;

        let text = std::fs::read_to_string(&index_path)
            .with_context(|| format!("read {}", index_path.display()))?;
        let mut kept: Vec<String> = Vec::new();
        let mut deleted_ids: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let parsed: std::result::Result<HistoryEntry, _> = serde_json::from_str(line);
            let Ok(e) = parsed else {
                // 解析不动的行原样留着, 不丢
                kept.push(line.to_string());
                continue;
            };
            let should_delete = e.ts.parse::<DateTime<Utc>>().is_ok_and(|t| t < cutoff);
            if should_delete {
                let id = if e.id.is_empty() {
                    legacy_id_from_ts(&e.ts)
                } else {
                    e.id.clone()
                };
                deleted_ids.push(id);
            } else {
                kept.push(line.to_string());
            }
        }
        let count = deleted_ids.len();
        if count == 0 {
            return Ok(0);
        }

        let tmp = index_path.with_extension("jsonl.tmp");
        let mut tmp_f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        for line in &kept {
            writeln!(tmp_f, "{line}").context("write tmp")?;
        }
        tmp_f.sync_all().context("sync tmp")?;
        drop(tmp_f);
        std::fs::rename(&tmp, &index_path)
            .with_context(|| format!("rename {} → {}", tmp.display(), index_path.display()))?;

        // 删全文文件
        for id in &deleted_ids {
            if let Some(p) = Self::full_path(id) {
                std::fs::remove_file(&p).ok();
            }
        }
        Ok(count)
    }

    /// 全量导出 (供用户 `> file` 重定向)。
    ///
    /// - `format = "jsonl"`: 原样导出索引文件 (内容跟 `cat ai_history.jsonl` 一样)
    /// - `format = "md"`: Markdown 表格 + 每条一段全文 (找不到全文文件就只导摘要)
    pub fn export(format: &str) -> Result<String> {
        let entries = Self::list(&ListFilter::default())?;
        match format {
            "jsonl" => {
                let mut out = String::new();
                for e in &entries {
                    let line = serde_json::to_string(e).context("serialize")?;
                    out.push_str(&line);
                    out.push('\n');
                }
                Ok(out)
            }
            "md" => Ok(render_md(&entries)),
            other => anyhow::bail!("unknown format `{other}` (支持 jsonl / md)"),
        }
    }
}

/// 老条目 (无 id) 用 ts 派生一个稳定的临时 id。
///
/// 格式: ts 里的 `2026-05-23T06:42:13.380879+00:00` → `20260523-064213-0000`。
/// 用户后续 show 这种 id 会拿不到全文文件 (因为老数据不存全文), 提示就行。
fn legacy_id_from_ts(ts: &str) -> HistoryId {
    let cleaned: String = ts.chars().filter(char::is_ascii_digit).take(14).collect();
    if cleaned.len() == 14 {
        format!("{}-{}-legacy", &cleaned[..8], &cleaned[8..14])
    } else {
        format!("legacy-{}", ts.chars().take(20).collect::<String>())
    }
}

fn render_md(entries: &[HistoryEntry]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "# AI ask history ({} 条)\n", entries.len());
    let _ = writeln!(out, "| 时间 | from → to | model | 状态 | 耗时 | id |");
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for e in entries {
        let model = e.model.as_deref().unwrap_or("-");
        let _ = writeln!(
            out,
            "| {} | {} → {} | {} | {} | {} ms | `{}` |",
            e.ts.split('.').next().unwrap_or(&e.ts),
            e.from,
            e.to,
            model,
            e.status,
            e.latency_ms,
            e.id,
        );
    }
    out.push_str("\n---\n\n");
    for e in entries {
        let _ = writeln!(out, "## {} — {} → {}\n", e.id, e.from, e.to);
        let _ = writeln!(out, "**时间**: {}  ", e.ts);
        let _ = writeln!(out, "**状态**: {} ({} ms)\n", e.status, e.latency_ms);
        // 试着拿全文
        if let Ok(full) = HistoryStore::show(&e.id) {
            let _ = writeln!(out, "### Q\n\n```\n{}\n```\n", full.prompt);
            let _ = writeln!(out, "### A\n\n```\n{}\n```\n", full.response);
        } else {
            let _ = writeln!(out, "### Q (摘要)\n\n```\n{}\n```\n", e.prompt_excerpt);
            let _ = writeln!(out, "### A (摘要)\n\n```\n{}\n```\n", e.response_excerpt);
        }
        if let Some(err) = &e.error {
            let _ = writeln!(out, "**错误**: `{err}`\n");
        }
        out.push_str("\n---\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use tempfile::TempDir;

    /// 全局锁: cargo test 默认多线程并跑, 改 HOME 的测试必须串行
    /// (否则一个测试还没读完文件, 另一个改了 HOME 把 dirs::home_dir() 指走).
    fn home_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        // 别人测试 panic 中毒锁也能拿到 (单测互不依赖, 中毒不致命)
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 临时改 HOME 让单测互不污染。拿全局 home_lock() 实现串行.
    struct HomeGuard {
        _td: TempDir,
        old: Option<std::ffi::OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new() -> Self {
            // 先拿全局锁, 再改 HOME, 保证一次只有一个测试用临时 HOME
            let lock = home_lock();
            let td = tempfile::tempdir().expect("tempdir");
            let old = std::env::var_os("HOME");
            std::env::set_var("HOME", td.path());
            Self {
                _td: td,
                old,
                _lock: lock,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(o) = &self.old {
                std::env::set_var("HOME", o);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    fn sample_entry(id: &str, to: &str, status: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.to_string(),
            ts: Utc::now().to_rfc3339(),
            from: "test".to_string(),
            to: to.to_string(),
            source_cwd: Some("/tmp/cwd".to_string()),
            source_tag: None,
            model: Some("sonnet".to_string()),
            prompt_excerpt: "Q...".to_string(),
            response_excerpt: "A...".to_string(),
            status: status.to_string(),
            error: None,
            latency_ms: 123,
        }
    }

    #[test]
    fn new_id_format() {
        let id = HistoryStore::new_id();
        // 长度: YYYYMMDD-HHMMSS-XXXX = 8+1+6+1+4 = 20
        assert_eq!(id.len(), 20);
        assert!(id.chars().nth(8) == Some('-'));
        assert!(id.chars().nth(15) == Some('-'));
    }

    #[test]
    fn append_and_list_roundtrip() {
        let _g = HomeGuard::new();
        let e1 = sample_entry(&HistoryStore::new_id(), "claude", "ok");
        let e2 = sample_entry(&HistoryStore::new_id(), "codex", "ok");
        HistoryStore::append(&e1, "full Q1", "full A1").unwrap();
        HistoryStore::append(&e2, "full Q2", "full A2").unwrap();
        let list = HistoryStore::list(&ListFilter::default()).unwrap();
        assert_eq!(list.len(), 2);
        // 最新在前, 所以 e2 第一个
        assert_eq!(list[0].to, "codex");
        assert_eq!(list[1].to, "claude");
    }

    #[test]
    fn list_filter_provider() {
        let _g = HomeGuard::new();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "claude", "ok"),
            "q",
            "a",
        )
        .unwrap();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "codex", "ok"),
            "q",
            "a",
        )
        .unwrap();
        let filter = ListFilter {
            provider: Some("claude".to_string()),
            ..Default::default()
        };
        let list = HistoryStore::list(&filter).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].to, "claude");
    }

    #[test]
    fn list_filter_status() {
        let _g = HomeGuard::new();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "claude", "ok"),
            "q",
            "a",
        )
        .unwrap();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "claude", "err"),
            "q",
            "a",
        )
        .unwrap();
        let filter = ListFilter {
            status: Some("err".to_string()),
            ..Default::default()
        };
        let list = HistoryStore::list(&filter).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, "err");
    }

    #[test]
    fn show_returns_full_record() {
        let _g = HomeGuard::new();
        let id = HistoryStore::new_id();
        let e = sample_entry(&id, "claude", "ok");
        HistoryStore::append(&e, "完整 prompt 全文", "完整 reply 全文").unwrap();
        let full = HistoryStore::show(&id).unwrap();
        assert_eq!(full.prompt, "完整 prompt 全文");
        assert_eq!(full.response, "完整 reply 全文");
    }

    #[test]
    fn show_missing_id_errors() {
        let _g = HomeGuard::new();
        let err = HistoryStore::show("20260101-000000-deaf").unwrap_err();
        assert!(format!("{err:#}").contains("找不到"));
    }

    #[test]
    fn delete_removes_index_line_and_full_file() {
        let _g = HomeGuard::new();
        let id = HistoryStore::new_id();
        HistoryStore::append(&sample_entry(&id, "claude", "ok"), "q", "a").unwrap();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "codex", "ok"),
            "q",
            "a",
        )
        .unwrap();
        HistoryStore::delete(&id).unwrap();
        let list = HistoryStore::list(&ListFilter::default()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].to, "codex");
        // 全文也没了
        assert!(!HistoryStore::full_path(&id).unwrap().exists());
    }

    #[test]
    fn delete_unknown_id_errors() {
        let _g = HomeGuard::new();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "claude", "ok"),
            "q",
            "a",
        )
        .unwrap();
        let err = HistoryStore::delete("nope-nope-nope").unwrap_err();
        assert!(format!("{err:#}").contains("没找到"));
    }

    #[test]
    fn export_jsonl_dumps_index() {
        let _g = HomeGuard::new();
        HistoryStore::append(
            &sample_entry(&HistoryStore::new_id(), "claude", "ok"),
            "q",
            "a",
        )
        .unwrap();
        let out = HistoryStore::export("jsonl").unwrap();
        assert!(out.contains("\"to\":\"claude\""));
    }

    #[test]
    fn export_md_renders_table_and_sections() {
        let _g = HomeGuard::new();
        let id = HistoryStore::new_id();
        HistoryStore::append(&sample_entry(&id, "claude", "ok"), "完整 Q", "完整 A").unwrap();
        let out = HistoryStore::export("md").unwrap();
        assert!(out.contains("# AI ask history"));
        assert!(out.contains("完整 Q"));
        assert!(out.contains("完整 A"));
    }

    #[test]
    fn legacy_id_from_iso_ts() {
        let id = legacy_id_from_ts("2026-05-23T06:42:13.380879+00:00");
        assert_eq!(id, "20260523-064213-legacy");
    }

    #[test]
    fn list_backfills_legacy_id() {
        let _g = HomeGuard::new();
        // 手写一条老格式 (无 id) 索引行
        let path = HistoryStore::index_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let line = r#"{"ts":"2026-05-23T06:42:13.380879+00:00","from":"x","to":"claude","prompt_excerpt":"q","response_excerpt":"a","status":"ok","latency_ms":1}"#;
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let list = HistoryStore::list(&ListFilter::default()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "20260523-064213-legacy");
    }

    /// D3: 10 个线程同时写 history, 验证索引文件不损坏.
    ///
    /// 没锁的话, 老 `O_APPEND` 在 Linux 文件系统层面是原子的, 但 Windows /
    /// 网络盘 / 大块写入不保证. 加 `fs2::FileExt::lock_exclusive` 后,
    /// 无论环境都安全 — 这个测试**就算今天通过**, 是对未来 Windows 用户的保障.
    ///
    /// 断言:
    /// - 索引文件最终行数 == 10
    /// - 每一行都是合法 JSON
    /// - 10 个 id 没重复 (各拿到独立短码)
    #[test]
    fn concurrent_append_10_threads_no_corruption() {
        use std::sync::Arc;
        use std::thread;

        let _g = HomeGuard::new();
        let barrier = Arc::new(std::sync::Barrier::new(10));
        let mut handles = Vec::new();
        for i in 0..10 {
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait(); // 同时开跑, 最大化并发压力
                let entry = sample_entry(&HistoryStore::new_id(), "claude", "ok");
                HistoryStore::append(
                    &entry,
                    &format!("Q from thread {i}"),
                    &format!("A from thread {i}"),
                )
                .expect("append should succeed under lock");
            }));
        }
        for h in handles {
            h.join().expect("thread should not panic");
        }
        // 读索引文件验证 — 必须正好 10 行, 每行合法 JSON, id 不重复
        let list = HistoryStore::list(&ListFilter::default()).expect("list");
        assert_eq!(list.len(), 10, "expected 10 entries after 10 concurrent appends");
        let ids: std::collections::HashSet<_> = list.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids.len(), 10, "ids should be unique");
    }
}
