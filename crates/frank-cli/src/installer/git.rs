//! Git 源码获取: clone / fetch / checkout 到 frank 本地 cache。
//!
//! # cache 布局
//!
//! ```text
//! ~/.frank/cache/
//!   <16-hex-of-sha256(url)>/   ← bare path 仅供调试; 实际是完整 working tree
//!     .git/
//!     <repo files...>
//! ```
//!
//! cache key 由 `url.trim().to_lowercase()` 的 SHA-256 前 8 字节 (16 hex 字符) 决定。
//! 同一 url 跨设备一致 (满足 ADR 设计中"分布式 cache 一致"诉求);
//! 16 hex (64 bit) 碰撞概率对 frank 的规模 (用户级 < 1000 skill) 可忽略。
//!
//! # 不做的事
//!
//! - 不开 sparse-checkout: doris-ops 等公开 skill 都很小, 实测 clone < 2s; 未来 kdwl 单仓
//!   多 skill 再加 sparse (P1 跟 subpath 优化一起做)
//! - 不暴露 fetch 进度回调: 安静模式; verbose 用 `RUST_LOG=frank::installer::git=debug` 看
//! - 不处理 GitHub PAT / SSH key: 由系统 ssh-agent / credential helper 接管

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::{build::CheckoutBuilder, build::RepoBuilder, FetchOptions, ProxyOptions, Repository};
use sha2::{Digest, Sha256};

/// Fetch 返回结果。
#[derive(Debug, Clone)]
pub struct FetchResult {
    /// 已 checkout 完毕的本地仓库根目录 (working tree)。
    pub repo_dir: PathBuf,
    /// 当前 HEAD 指向的 commit SHA (40 hex 字符)。
    pub commit_sha: String,
}

/// cache 根目录: `~/.frank/cache/`。
pub fn cache_root() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate user home dir")?
        .join(".frank")
        .join("cache"))
}

/// 某 url 对应的 cache 目录 (不会创建)。
///
/// 跨设备一致: key = `first16chars(hex(sha256(url.trim().lowercase())))`。
pub fn cache_dir_for(url: &str) -> Result<PathBuf> {
    Ok(cache_root()?.join(url_cache_key(url)))
}

/// 计算 url 的 cache key (导出供测试用)。
#[must_use]
pub fn url_cache_key(url: &str) -> String {
    let normalized = url.trim().to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex_encode(&digest[..8])
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // 写 String 不会失败, expect 是更直白的契约表达
        write!(&mut s, "{b:02x}").expect("write to String never fails");
    }
    s
}

/// 拉取或更新 `url` 到 cache, 把 working tree checkout 到 `git_ref` (branch / tag / sha)。
///
/// # 行为
/// - cache 已存在且是合法 repo: `fetch origin` 然后 checkout `FETCH_HEAD`
/// - cache 不存在或损坏: 清掉 → `git clone url` → `fetch + checkout`
///
/// # 返回
/// 实际 working tree 路径与 commit SHA。
pub fn fetch(url: &str, git_ref: &str) -> Result<FetchResult> {
    let dest = cache_dir_for(url)?;
    tracing::debug!(url, git_ref, dest = %dest.display(), "git fetch start");

    let repo = open_or_clone(&dest, url)?;
    fetch_and_checkout(&repo, git_ref)?;

    let head = repo.head().context("read HEAD")?;
    let commit = head.peel_to_commit().context("peel HEAD to commit")?;
    let sha = commit.id().to_string();

    tracing::debug!(sha = %sha, "git fetch done");
    Ok(FetchResult {
        repo_dir: dest,
        commit_sha: sha,
    })
}

/// 构造启用代理的 `FetchOptions`. 两层 fallback:
///
/// 1. `~/.frank/config.toml [proxy].http` (显式 URL, daemon 模式必走这条 —
///    launchd 启动的 daemon 不继承 user shell 的 HTTP_PROXY env)
/// 2. `ProxyOptions::auto()` 让 libgit2 自动读 HTTP_PROXY / HTTPS_PROXY env
///    (user 终端直接跑 frank install 时还是这条)
///
/// 每个 fetch 调用都需要单独构造一个 (FetchOptions 持有的回调状态不可复用)。
fn build_fetch_options<'cb>() -> FetchOptions<'cb> {
    let mut proxy = ProxyOptions::new();
    if let Some(url) = read_config_proxy() {
        proxy.url(&url);
    } else {
        proxy.auto();
    }
    let mut fo = FetchOptions::new();
    fo.proxy_options(proxy);
    fo
}

/// 读 `~/.frank/config.toml [proxy].http`, 没配返回 None.
///
/// 跟 `frank config set-proxy` 写入的 schema 对齐 (http / https / all 三个字段同值).
fn read_config_proxy() -> Option<String> {
    let path = dirs::home_dir()?.join(".frank").join("config.toml");
    let text = fs::read_to_string(&path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    v.get("proxy")?
        .get("http")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}

fn open_or_clone(dest: &Path, url: &str) -> Result<Repository> {
    if dest.join(".git").exists() {
        Repository::open(dest).with_context(|| format!("open cache repo {}", dest.display()))
    } else {
        if dest.exists() {
            tracing::warn!(path = %dest.display(), "stale cache (no .git) — removing");
            fs::remove_dir_all(dest)
                .with_context(|| format!("remove stale cache {}", dest.display()))?;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir cache parent {}", parent.display()))?;
        }
        let mut builder = RepoBuilder::new();
        builder.fetch_options(build_fetch_options());
        builder
            .clone(url, dest)
            .with_context(|| format!("clone {url} -> {}", dest.display()))
    }
}

fn fetch_and_checkout(repo: &Repository, git_ref: &str) -> Result<()> {
    let mut remote = repo.find_remote("origin").context("find remote 'origin'")?;
    let mut fo = build_fetch_options();
    remote
        .fetch(&[git_ref], Some(&mut fo), None)
        .with_context(|| format!("fetch ref {git_ref} from origin"))?;

    let fetch_head = repo
        .find_reference("FETCH_HEAD")
        .context("locate FETCH_HEAD after fetch")?;
    let commit = fetch_head
        .peel_to_commit()
        .context("peel FETCH_HEAD to commit")?;
    let tree = commit.tree().context("read commit tree")?;

    let mut opts = CheckoutBuilder::new();
    opts.force();
    repo.checkout_tree(tree.as_object(), Some(&mut opts))
        .with_context(|| format!("checkout tree at {}", commit.id()))?;
    repo.set_head_detached(commit.id())
        .with_context(|| format!("set HEAD -> {}", commit.id()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_cache_key_is_deterministic() {
        let k1 = url_cache_key("https://github.com/x/y.git");
        let k2 = url_cache_key("https://github.com/x/y.git");
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), 16); // 8 bytes hex = 16 chars
    }

    #[test]
    fn url_cache_key_normalizes_case_and_whitespace() {
        let a = url_cache_key("https://GitHub.com/x/y.git");
        let b = url_cache_key("  https://github.com/x/y.git  ");
        assert_eq!(a, b);
    }

    #[test]
    fn url_cache_key_distinguishes_different_urls() {
        let a = url_cache_key("https://github.com/x/a.git");
        let b = url_cache_key("https://github.com/x/b.git");
        assert_ne!(a, b);
    }
}
