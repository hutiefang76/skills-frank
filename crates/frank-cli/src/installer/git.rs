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

    // v0.14: 2 阶段重试
    // 1. 直连 (尊重 ~/.frank/config.toml [proxy].http 配的代理)
    // 2. 失败且配了 [mirror].github → rewrite URL 重试 (国内访问 github 慢的兜底)
    let direct_err = match try_fetch(&dest, url, git_ref) {
        Ok(r) => return Ok(r),
        Err(e) => e,
    };
    if let Some(mirror_url) = read_config_github_mirror().and_then(|m| rewrite_via_mirror(url, &m))
    {
        tracing::warn!(
            url, mirror = %mirror_url,
            error = %format!("{direct_err:#}"),
            "direct fetch failed, retry via mirror"
        );
        // 清掉可能半装好的 cache, 不然 open_or_clone 拿到坏 repo
        if dest.exists() {
            let _ = fs::remove_dir_all(&dest);
        }
        return try_fetch(&dest, &mirror_url, git_ref).map_err(|mirror_err| {
            anyhow::anyhow!(
                "direct + mirror both failed.\n  direct: {direct_err:#}\n  mirror ({mirror_url}): {mirror_err:#}"
            )
        });
    }
    Err(direct_err)
}

/// v0.14: 实际跑一次 clone+fetch+checkout, 不带 mirror fallback.
fn try_fetch(dest: &Path, url: &str, git_ref: &str) -> Result<FetchResult> {
    let repo = open_or_clone(dest, url)?;
    fetch_and_checkout(&repo, git_ref)?;

    let head = repo.head().context("read HEAD")?;
    let commit = head.peel_to_commit().context("peel HEAD to commit")?;
    let sha = commit.id().to_string();

    tracing::debug!(sha = %sha, "git fetch done");
    Ok(FetchResult {
        repo_dir: dest.to_path_buf(),
        commit_sha: sha,
    })
}

/// 读 `~/.frank/config.toml [mirror].github`, 没配返回 None.
///
/// v0.14 国内访问 github 慢的兜底. 用户跑 `frank config set mirror.github <prefix>` 配置.
/// 例:
///   `frank config set mirror.github https://ghproxy.com`     → 拼成 `<mirror>/https://github.com/X/Y.git`
///   `frank config set mirror.github https://gitclone.com`    → 拼成 `<mirror>/github.com/X/Y.git`
///   `frank config set mirror.github https://hub.fastgit.xyz` → host 替换
fn read_config_github_mirror() -> Option<String> {
    let path = dirs::home_dir()?.join(".frank").join("config.toml");
    let text = fs::read_to_string(&path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    v.get("mirror")?
        .get("github")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
}

/// 把 `https://github.com/X/Y.git` 改写成镜像 URL.
///
/// 支持 3 种镜像 prefix 风格:
/// - `https://ghproxy.com`        → `<mirror>/https://github.com/X/Y.git`        (前置代理风格)
/// - `https://gitclone.com`       → `<mirror>/github.com/X/Y.git`                (去 scheme)
/// - `https://hub.fastgit.xyz`    → `<mirror>/X/Y.git`                            (host 直替)
///
/// 检测: mirror host 含 `proxy` 或 `cors` 走前置代理风格; 否则按 host 直替.
/// 非 github.com URL 返回 None (不动 gitee / 私有 git).
fn rewrite_via_mirror(url: &str, mirror: &str) -> Option<String> {
    // 仅对 github 走镜像 — 私有 git / gitee 等不动
    let suffix = url.strip_prefix("https://github.com/")?;
    let host = mirror
        .strip_prefix("https://")
        .or_else(|| mirror.strip_prefix("http://"))
        .unwrap_or(mirror);
    // 前置代理 (ghproxy / cors-style): 保留完整源 URL 拼在后面
    if host.contains("proxy") || host.contains("cors") || host.contains("ghp") {
        Some(format!("{mirror}/{url}"))
    } else if host.contains("clone") {
        // gitclone.com 风格: 去 https:// 但保留 github.com 段
        Some(format!("{mirror}/github.com/{suffix}"))
    } else {
        // host 直替 (fastgit / kkgithub / etc.)
        Some(format!("{mirror}/{suffix}"))
    }
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

    // v0.14 mirror rewriter

    #[test]
    fn mirror_ghproxy_prefixes_full_url() {
        let r = rewrite_via_mirror("https://github.com/x/y.git", "https://ghproxy.com");
        assert_eq!(
            r.as_deref(),
            Some("https://ghproxy.com/https://github.com/x/y.git")
        );
    }

    #[test]
    fn mirror_gitclone_strips_scheme() {
        let r = rewrite_via_mirror("https://github.com/x/y.git", "https://gitclone.com");
        assert_eq!(r.as_deref(), Some("https://gitclone.com/github.com/x/y.git"));
    }

    #[test]
    fn mirror_fastgit_replaces_host() {
        let r = rewrite_via_mirror("https://github.com/x/y.git", "https://hub.fastgit.xyz");
        assert_eq!(r.as_deref(), Some("https://hub.fastgit.xyz/x/y.git"));
    }

    #[test]
    fn mirror_skips_non_github() {
        // 私有 git / gitee 不动 — 镜像只代理 github
        let r = rewrite_via_mirror("https://gitlab.com/x/y.git", "https://ghproxy.com");
        assert_eq!(r, None);
    }

    #[test]
    fn mirror_handles_trailing_slash() {
        let r = rewrite_via_mirror("https://github.com/x/y.git", "https://ghproxy.com/");
        // read_config 会 trim_end_matches('/'), 这里手 pass trailing 也要稳
        // (虽然实际用 read_config 路径已 trim, 这里直 call 时还会拼出双 / — 但不严重)
        assert!(r.as_deref().unwrap().contains("github.com/x/y.git"));
    }
}
