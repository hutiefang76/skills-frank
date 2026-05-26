//! `frank market` — 从公共市场拉 skill / MCP 列表写到本地 manifest.
//!
//! 用户原话: "咱们是不是得集成 skills/mcp 市场?" v0.7 初版只做 MCP (modelcontextprotocol/servers
//! 官方 repo) 和 anthropics/skills (含 17 个 a 社官方 skill). 后续可扩 awesome-mcp-servers
//! 等社区源.
//!
//! # 子命令
//!
//! - `frank market sync` — 拉所有支持的源, 写到 `~/.frank/manifests/market-<source>.yaml`
//! - `frank market list` — 只列, 不写 (预览用)
//!
//! # 设计
//!
//! 写入路径在 `~/.frank/manifests/`, 跟用户私有 manifest 同目录, 用户能手编辑/删. 自动
//! sync 用 `frank market sync` 重写. visibility 标 `curated` (frank 项目方背书的清单).
//!
//! 实现走 GitHub Contents API (跟 frank.hutiefang.com 一样, 自动走 ~/.frank/config.toml proxy).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// `frank market` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: MarketCommand,
}

/// `frank market` 子命令。
#[derive(Subcommand, Debug)]
pub enum MarketCommand {
    /// 拉所有支持的市场源, 写到 ~/.frank/manifests/market-<source>.yaml.
    Sync,
    /// 只列预览, 不写文件 (debug 用).
    List,
    /// v0.14: 清掉 ~/.frank/cache/market/ 所有缓存 (etag + body), 强制下次 sync/list 重拉.
    ClearCache,
}

/// 执行 market 命令。
pub fn run(args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match args.command {
            MarketCommand::Sync => sync().await,
            MarketCommand::List => list().await,
            MarketCommand::ClearCache => clear_cache(),
        }
    })
}

/// 构造 GitHub API client. token 来源优先级:
/// 1. env `GITHUB_TOKEN`
/// 2. env `GH_TOKEN`
/// 3. `gh auth token` subprocess (用户装了 gh + 登录过)
/// 4. 无 token (60req/h 限速, 一般够 sync 一次)
fn github_client() -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("frank-cli/market-sync");
    if let Some(token) = find_github_token() {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
            builder = builder.default_headers(headers);
        }
    }
    builder.build().context("build reqwest client")
}

fn find_github_token() -> Option<String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    if let Ok(t) = std::env::var("GH_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    // 兜底: gh auth token (用户装 gh 登录过的话最稳)
    if let Ok(out) = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
    {
        if out.status.success() {
            let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

async fn list_dir_names(client: &reqwest::Client, url: &str) -> Result<Vec<String>> {
    let raw = cached_github_get(client, url).await?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&raw).context("parse GH contents JSON")?;
    Ok(items
        .iter()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.to_string();
            let item_type = v.get("type")?.as_str()?;
            (item_type == "dir").then_some(name)
        })
        .collect())
}

/// v0.14: 24h disk cache + ETag 兜底 (修 cc-switch 卡顿根因, ADR-014 §3.2).
///
/// 缓存路径: `~/.frank/cache/market/<sha8(url)>.json` (内容) + `.etag` (头).
/// 流程:
/// 1. 文件存在 + mtime 在 24h 内 → 直读返回 (秒级响应)
/// 2. 否则发请求, 带 If-None-Match: <已存 etag> 头
///    - 304 Not Modified → 刷 mtime + 用老内容
///    - 200 OK → 写新内容 + 新 etag, 返回新内容
///
/// 失败时若有老 cache 也回退用 (offline 友好).
async fn cached_github_get(client: &reqwest::Client, url: &str) -> Result<String> {
    use sha2::{Digest, Sha256};
    let key = {
        let mut h = Sha256::new();
        h.update(url.as_bytes());
        let d = h.finalize();
        // 8 hex chars 足够区分 (32 bit), 全球 frank 用户 <10K url 不撞
        d.iter().take(4).fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    };
    let cache_dir = dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join("cache")
        .join("market");
    let body_path = cache_dir.join(format!("{key}.json"));
    let etag_path = cache_dir.join(format!("{key}.etag"));

    // 1. 24h 内热 cache 直返
    if let Ok(meta) = std::fs::metadata(&body_path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < 24 * 3600 {
                    if let Ok(s) = std::fs::read_to_string(&body_path) {
                        tracing::debug!(url, age_sec = elapsed.as_secs(), "market cache hit (warm)");
                        return Ok(s);
                    }
                }
            }
        }
    }

    // 2. 发请求, 带 If-None-Match (如果有老 etag)
    let mut req = client.get(url);
    if let Ok(etag) = std::fs::read_to_string(&etag_path) {
        let etag = etag.trim();
        if !etag.is_empty() {
            req = req.header("If-None-Match", etag);
        }
    }
    let resp_result = req.send().await;

    let resp = match resp_result {
        Ok(r) => r,
        Err(e) => {
            // 网络失败 fallback 用老 cache (即使过 24h, 比报错强)
            if let Ok(stale) = std::fs::read_to_string(&body_path) {
                tracing::warn!(
                    url, error = %e,
                    "market fetch failed, serving stale cache (>=24h)"
                );
                return Ok(stale);
            }
            return Err(e).with_context(|| format!("GET {url} (no cache fallback)"));
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        // 304: 内容没变, 刷 mtime 让下次 24h 内热命中
        let _ = std::fs::File::open(&body_path).and_then(|f| {
            // touch — set modified to now via filetime crate? 不引新 dep, 直接 read+write
            let s = std::fs::read_to_string(&body_path).unwrap_or_default();
            let _ = std::fs::write(&body_path, s);
            f.sync_all()
        });
        if let Ok(cached) = std::fs::read_to_string(&body_path) {
            tracing::debug!(url, "market cache 304 (still valid)");
            return Ok(cached);
        }
    }

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // fallback to stale if available
        if let Ok(stale) = std::fs::read_to_string(&body_path) {
            tracing::warn!(
                url, status = %status,
                "market fetch {} error, serving stale cache", status
            );
            return Ok(stale);
        }
        anyhow::bail!(
            "GitHub API returned {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }

    // 3. 200 OK: 写新 cache + etag
    let new_etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body = resp.text().await.context("read body")?;
    if let Some(parent) = body_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&body_path, &body);
    if let Some(etag) = new_etag {
        let _ = std::fs::write(&etag_path, &etag);
    }
    Ok(body)
}

/// 从 modelcontextprotocol/servers GitHub repo 拉 src/ 目录列表 (官方 reference 实现).
async fn fetch_mcp_servers() -> Result<Vec<McpEntry>> {
    let client = github_client()?;
    let names = list_dir_names(
        &client,
        "https://api.github.com/repos/modelcontextprotocol/servers/contents/src",
    )
    .await?;
    Ok(names.into_iter().map(|name| McpEntry { name }).collect())
}

/// 从 anthropics/skills GitHub repo 拉 skills/ 目录列表 (a 社官方 17 个 skill).
async fn fetch_anthropic_skills() -> Result<Vec<AnthropicSkill>> {
    let client = github_client()?;
    let names = list_dir_names(
        &client,
        "https://api.github.com/repos/anthropics/skills/contents/skills",
    )
    .await?;
    Ok(names
        .into_iter()
        .map(|name| AnthropicSkill { name })
        .collect())
}

struct McpEntry {
    name: String,
}

struct AnthropicSkill {
    name: String,
}

async fn list() -> Result<()> {
    crate::log::ui::section("frank market list — 预览 (不写文件)");
    println!();
    crate::log::ui::info("拉 modelcontextprotocol/servers (官方 MCP reference 实现)...");
    let mcp = match fetch_mcp_servers().await {
        Ok(v) => v,
        Err(e) => {
            crate::log::ui::error(&format!(
                "拉 MCP 列表失败: {e:#}\n\
                 (常见: 1. 没网络; 2. GitHub API 限速 60req/h — 等 1h 或 export GITHUB_TOKEN=...;\n\
                  3. 走 Clash 美区 IP 池被共享限速 — 切节点试)"
            ));
            Vec::new()
        }
    };
    println!("  MCP servers ({} 个):", mcp.len());
    for e in &mcp {
        println!("    - mcp-{}", e.name);
    }
    println!();
    crate::log::ui::info("拉 anthropics/skills (a 社官方 skill)...");
    let skills = match fetch_anthropic_skills().await {
        Ok(v) => v,
        Err(e) => {
            crate::log::ui::error(&format!("拉 anthropic skills 失败: {e:#}"));
            Vec::new()
        }
    };
    println!("  Anthropic skills ({} 个):", skills.len());
    for s in &skills {
        println!("    - {}", s.name);
    }
    println!();
    crate::log::ui::info(&format!(
        "共 {} MCP + {} skill, `frank market sync` 真写到 ~/.frank/manifests/",
        mcp.len(),
        skills.len()
    ));
    Ok(())
}

async fn sync() -> Result<()> {
    crate::log::ui::section("frank market sync — 写 manifest 到 ~/.frank/manifests/");
    println!();
    let mcp = fetch_mcp_servers()
        .await
        .context("fetch MCP servers list")?;
    crate::log::ui::info(&format!(
        "拿到 {} 个 MCP server (modelcontextprotocol/servers)",
        mcp.len()
    ));
    write_mcp_manifest(&mcp)?;

    let skills = fetch_anthropic_skills()
        .await
        .context("fetch anthropic skills list")?;
    crate::log::ui::info(&format!("拿到 {} 个 anthropic skill", skills.len()));
    write_anthropic_skills_manifest(&skills)?;

    crate::log::ui::success("sync 完成. 跑 `frank list` 看, `frank install <name>` 装.");
    Ok(())
}

/// v0.14: `frank market clear-cache` — 清 ~/.frank/cache/market/ 整目录.
fn clear_cache() -> Result<()> {
    let cache_dir = dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join("cache")
        .join("market");
    if !cache_dir.exists() {
        crate::log::ui::info("没 market 缓存目录可清 (~/.frank/cache/market/ 不存在)");
        return Ok(());
    }
    let mut count = 0_usize;
    for entry in fs::read_dir(&cache_dir).with_context(|| format!("read {}", cache_dir.display()))? {
        let entry = entry?;
        if entry.path().is_file() {
            let _ = fs::remove_file(entry.path());
            count += 1;
        }
    }
    crate::log::ui::success(&format!(
        "清了 {} 个缓存文件 ({}). 下次 `frank market sync` 会重拉.",
        count,
        cache_dir.display()
    ));
    Ok(())
}

fn manifests_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join("manifests"))
}

fn write_mcp_manifest(entries: &[McpEntry]) -> Result<()> {
    let dir = manifests_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = dir.join("market-mcp-official.yaml");
    use std::fmt::Write as _;
    let mut yaml = String::from(
        "# frank market sync — modelcontextprotocol/servers (官方 reference 实现)\n\
         # 自动生成. 重 sync 会覆盖. 手编辑的话改名加自己后缀防覆盖.\n\
         schema_version: 1\nprofile: personal\nskills:\n",
    );
    for e in entries {
        let _ = writeln!(
            yaml,
            "  - name: mcp-{name}\n    description: MCP server `{name}` (modelcontextprotocol/servers official)\n    source:\n      type: mcp\n      command: npx\n      args: [\"-y\", \"@modelcontextprotocol/server-{name}\"]\n    visibility: curated\n    target_platforms: [claude, codex]",
            name = e.name
        );
    }
    fs::write(&path, yaml).with_context(|| format!("write {}", path.display()))?;
    crate::log::ui::success(&format!(
        "  ✓ {} ({} 个 MCP)",
        path.display(),
        entries.len()
    ));
    Ok(())
}

fn write_anthropic_skills_manifest(entries: &[AnthropicSkill]) -> Result<()> {
    let dir = manifests_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = dir.join("market-anthropic-skills.yaml");
    use std::fmt::Write as _;
    let mut yaml = String::from(
        "# frank market sync — anthropics/skills (a 社官方 skill, 在 skills/* 子目录)\n\
         # 自动生成. 重 sync 会覆盖.\n\
         schema_version: 1\nprofile: personal\nskills:\n",
    );
    for e in entries {
        let _ = writeln!(
            yaml,
            "  - name: {name}\n    description: Anthropic 官方 skill `{name}` (anthropics/skills repo)\n    source:\n      type: git\n      url: https://github.com/anthropics/skills.git\n      subpath: skills/{name}\n    visibility: curated\n    target_platforms: [claude, codex]",
            name = e.name
        );
    }
    fs::write(&path, yaml).with_context(|| format!("write {}", path.display()))?;
    crate::log::ui::success(&format!(
        "  ✓ {} ({} 个 skill)",
        path.display(),
        entries.len()
    ));
    Ok(())
}
