//! `frank config` — 配置管理 (proxy 等).
//!
//! 用户原话: "为了保证兼容性。直接安装 检查是否存在 clash 或者其他常见的 vpn,
//! 尝试自动加载 也可以设置不使用代理? 然后设置的范围: frank 程序的安装升级? 还可以修改?"
//!
//! # 子命令
//!
//! - `frank config show`              — 看当前 ~/.frank/config.toml 内容
//! - `frank config get <key>`         — 读单个键 (例 `sync.agent_url`); v0.10.10 加
//! - `frank config set <key> <value>` — 写单个键 (dot-path, 例 `sync.agent_url`); v0.10.10 加
//! - `frank config unset <key>`       — 删单个键; v0.10.10 加
//! - `frank config detect-proxy`      — 自动扫本机常见代理 (Clash/Surge/...) 写入
//! - `frank config set-proxy <url>`   — 手敲 proxy URL 写入
//! - `frank config unset-proxy`       — 清掉 proxy 配置
//!
//! # 作用域 (config.toml [proxy])
//!
//! 当前生效:
//! - `frank ai ask` spawn 的 AI CLI 子进程 (claude/codex/opencode/gemini)
//! - frank orchestrator Web UI 提交的 job (orchestrator daemon spawn 子进程)
//!
//! 暂时不影响 (留 v0.7):
//! - `frank install` git clone 走代理 (要改 git2 配置)
//! - `brew install frank` / `cargo install` (由 brew/cargo 各自配置)

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

/// `frank config` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// `frank config` 子命令。
#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// 看当前 ~/.frank/config.toml 内容 (proxy / sync 等).
    Show,

    /// 读单个键 (例 `frank config get sync.agent_url`); v0.10.10 加.
    Get {
        /// 配置键 (dot-path, 例 `sync.agent_url` / `proxy.http`).
        key: String,
    },

    /// 写单个键 (例 `frank config set sync.agent_url http://localhost:8318`); v0.10.10 加.
    ///
    /// 已知有效键:
    /// - `sync.agent_url` — sync-agent base URL (例 `http://localhost:8318`)
    /// - `proxy.http` / `proxy.https` / `proxy.all` — HTTP 代理 URL
    /// - `proxy.no` — 不走代理的域名列表 (逗号分隔)
    ///
    /// 不在列表的键也能写, 但 frank 自己不读 (留给未来扩展).
    Set {
        /// 配置键 (dot-path).
        key: String,
        /// 值 (字符串).
        value: String,
    },

    /// 删单个键 (例 `frank config unset sync.agent_url`, 删除后回退默认值); v0.10.10 加.
    Unset {
        /// 配置键 (dot-path).
        key: String,
    },

    /// 自动检测本机常见代理 (Clash/Surge/V2ray 等), 写入 ~/.frank/config.toml.
    DetectProxy,

    /// 手敲 proxy URL 写入 (例 `--url http://127.0.0.1:7897`).
    SetProxy {
        /// proxy URL (http/https/all 都用同一个).
        #[arg(long)]
        url: String,

        /// 不走代理的域名列表 (逗号分隔, 默认 localhost,127.0.0.1,::1,.local).
        #[arg(long, default_value = "localhost,127.0.0.1,::1,.local")]
        no: String,
    },

    /// 清掉 proxy 配置 (子进程将不走代理).
    UnsetProxy,
}

/// 执行 config 命令。
pub fn run(args: Args) -> Result<()> {
    match args.command {
        ConfigCommand::Show => show(),
        ConfigCommand::Get { key } => get_key(&key),
        ConfigCommand::Set { key, value } => set_key(&key, &value),
        ConfigCommand::Unset { key } => unset_key(&key),
        ConfigCommand::DetectProxy => detect_proxy(),
        ConfigCommand::SetProxy { url, no } => set_proxy(&url, &no),
        ConfigCommand::UnsetProxy => unset_proxy(),
    }
}

/// 读单个键 (dot-path), 例 `sync.agent_url` → 进 [sync] 节读 agent_url.
fn get_key(key: &str) -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!("{} 不存在", path.display()));
        return Ok(());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let v: toml::Value = text.parse().context("parse toml")?;
    if let Some(val) = get_dotpath(&v, key) {
        println!("{}", val_to_string(val));
    } else {
        crate::log::ui::warn(&format!("键 `{key}` 不存在 (会用默认值)"));
    }
    Ok(())
}

/// 写单个键 (dot-path). 自动建中间 table.
fn set_key(key: &str, value: &str) -> Result<()> {
    if key.trim().is_empty() {
        anyhow::bail!("键不能为空");
    }
    // 简易校验 sync.agent_url 是否像 URL (其他键不强校)
    if key == "sync.agent_url"
        && !value.starts_with("http://")
        && !value.starts_with("https://")
    {
        anyhow::bail!("sync.agent_url 必须以 http:// 或 https:// 开头, 收到 `{value}`");
    }

    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut v: toml::Value = existing
        .parse()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    set_dotpath(&mut v, key, toml::Value::String(value.to_string()))?;
    let text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;

    crate::log::ui::success(&format!("{} = `{}` 写入 {}", key, value, path.display()));
    if key == "sync.agent_url" {
        crate::log::ui::info("frank memory / orchestrator 下次执行会用新地址");
    }
    Ok(())
}

/// 删单个键 (dot-path).
fn unset_key(key: &str) -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!("{} 不存在", path.display()));
        return Ok(());
    }
    let text = fs::read_to_string(&path).context("read config.toml")?;
    let mut v: toml::Value = text.parse().context("parse toml")?;
    if !unset_dotpath(&mut v, key) {
        crate::log::ui::warn(&format!("键 `{key}` 不存在"));
        return Ok(());
    }
    let new_text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, new_text).with_context(|| format!("write {}", path.display()))?;
    crate::log::ui::success(&format!("`{key}` 已删 (回退默认值)"));
    Ok(())
}

// ---- dot-path helpers (sync.agent_url → [sync].agent_url) ----

/// 读取 dot-path 对应的值 (找不到返回 None).
fn get_dotpath<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = root;
    for seg in key.split('.') {
        cur = cur.as_table()?.get(seg)?;
    }
    Some(cur)
}

/// 按 dot-path 写值, 中间 table 自动建.
fn set_dotpath(root: &mut toml::Value, key: &str, value: toml::Value) -> Result<()> {
    let segs: Vec<&str> = key.split('.').collect();
    if segs.iter().any(|s| s.is_empty()) {
        anyhow::bail!("键格式错: `{key}` (段不能空)");
    }
    let table = root
        .as_table_mut()
        .context("config root 必须是 table")?;
    if segs.len() == 1 {
        table.insert(segs[0].into(), value);
        return Ok(());
    }
    let mut cur = table
        .entry(segs[0].to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    for seg in &segs[1..segs.len() - 1] {
        let next = cur
            .as_table_mut()
            .with_context(|| format!("`{seg}` 不是 table, 不能下钻"))?;
        cur = next
            .entry((*seg).to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    }
    cur.as_table_mut()
        .context("末段不是 table")?
        .insert(segs[segs.len() - 1].into(), value);
    Ok(())
}

/// 按 dot-path 删值. 返回是否真删了.
fn unset_dotpath(root: &mut toml::Value, key: &str) -> bool {
    let segs: Vec<&str> = key.split('.').collect();
    let Some(table) = root.as_table_mut() else {
        return false;
    };
    if segs.len() == 1 {
        return table.remove(segs[0]).is_some();
    }
    let Some(mut cur) = table.get_mut(segs[0]) else {
        return false;
    };
    for seg in &segs[1..segs.len() - 1] {
        let Some(next) = cur.as_table_mut().and_then(|t| t.get_mut(*seg)) else {
            return false;
        };
        cur = next;
    }
    cur.as_table_mut()
        .is_some_and(|t| t.remove(segs[segs.len() - 1]).is_some())
}

/// toml::Value → 显示用字符串 (去字符串引号, 其他类型 to_string).
fn val_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn config_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("locate home dir")?
        .join(".frank")
        .join("config.toml"))
}

fn show() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn(&format!(
            "{} 不存在 (frank config detect-proxy 自动配)",
            path.display()
        ));
        return Ok(());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    crate::log::ui::section(&format!("frank config ({})", path.display()));
    println!("{text}");
    Ok(())
}

fn set_proxy(url: &str, no: &str) -> Result<()> {
    let url = url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("socks5://")
    {
        anyhow::bail!("proxy URL 必须以 http:// / https:// / socks5:// 开头, 收到: `{url}`");
    }
    write_proxy(url, no)?;
    crate::log::ui::success(&format!(
        "proxy 写入 {} (http/https/all = {url})",
        config_path()?.display()
    ));
    crate::log::ui::info("重启 daemon 让新 proxy 生效: brew services restart frank");
    Ok(())
}

fn unset_proxy() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        crate::log::ui::warn("config.toml 不存在, 没 proxy 可清");
        return Ok(());
    }
    let text = fs::read_to_string(&path).context("read config.toml")?;
    let mut v = text
        .parse::<toml::Value>()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    if let Some(t) = v.as_table_mut() {
        t.remove("proxy");
    }
    let new_text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, new_text).context("write config.toml")?;
    crate::log::ui::success("proxy 已清 (子进程将不走代理)");
    Ok(())
}

fn write_proxy(url: &str, no: &str) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut v: toml::Value = existing
        .parse()
        .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    let table = v.as_table_mut().expect("toml root is table");
    let mut proxy = toml::map::Map::new();
    proxy.insert("http".into(), toml::Value::String(url.into()));
    proxy.insert("https".into(), toml::Value::String(url.into()));
    proxy.insert("all".into(), toml::Value::String(url.into()));
    proxy.insert("no".into(), toml::Value::String(no.into()));
    table.insert("proxy".into(), toml::Value::Table(proxy));
    let text = toml::to_string_pretty(&v).context("serialize toml")?;
    fs::write(&path, text).context("write config.toml")
}

fn detect_proxy() -> Result<()> {
    crate::log::ui::section("frank config detect-proxy — 扫本机常见代理");
    // 常见代理端口 (按使用率排)
    let candidates: &[(u16, &str)] = &[
        (7897, "Clash Verge / Mihomo (新版默认)"),
        (7890, "Clash X / Clash Premium (老版默认)"),
        (1087, "ShadowsocksX-NG / V2rayU (老 macOS 默认)"),
        (6152, "Surge"),
        (10809, "v2rayN (Windows)"),
        (8080, "Charles / Fiddler 调试代理"),
        (8118, "Privoxy"),
    ];
    let mut found = Vec::new();
    for (port, label) in candidates {
        if probe_port(*port) {
            found.push((*port, *label));
            crate::log::ui::info(&format!("  ✓ 127.0.0.1:{port} ({label})"));
        }
    }
    if found.is_empty() {
        crate::log::ui::warn(
            "没扫到任何已知代理端口 — 手动配: frank config set-proxy --url http://...",
        );
        return Ok(());
    }
    let (port, label) = found[0];
    crate::log::ui::info(&format!("选第一个候选: 127.0.0.1:{port} ({label})"));
    let url = format!("http://127.0.0.1:{port}");
    write_proxy(&url, "localhost,127.0.0.1,::1,.local")?;
    crate::log::ui::success(&format!(
        "proxy 写入 {} (http/https/all = {url})",
        config_path()?.display()
    ));
    if found.len() > 1 {
        crate::log::ui::info(&format!(
            "(另有 {} 个候选, 想换跑 `frank config set-proxy --url http://127.0.0.1:<port>`)",
            found.len() - 1
        ));
    }
    crate::log::ui::info("重启 daemon 让新 proxy 生效: brew services restart frank");
    Ok(())
}

/// 探一下 127.0.0.1:port 是否可 TCP connect (代理客户端常在 localhost 监听).
fn probe_port(port: u16) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::time::Duration;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dot-path set 写入嵌套 table; get 能取回.
    #[test]
    fn set_get_dotpath_roundtrip() {
        let mut v: toml::Value = toml::Value::Table(toml::map::Map::new());
        set_dotpath(&mut v, "sync.agent_url", toml::Value::String("http://x:1".into()))
            .expect("set");
        let got = get_dotpath(&v, "sync.agent_url").expect("get");
        assert_eq!(val_to_string(got), "http://x:1");
    }

    /// 单层 key (无 dot) 也能 set/get.
    #[test]
    fn set_get_top_level_key() {
        let mut v: toml::Value = toml::Value::Table(toml::map::Map::new());
        set_dotpath(&mut v, "foo", toml::Value::String("bar".into())).expect("set");
        assert_eq!(val_to_string(get_dotpath(&v, "foo").unwrap()), "bar");
    }

    /// unset 删掉嵌套键, 后续 get 拿不到.
    #[test]
    fn unset_dotpath_removes_nested_key() {
        let mut v: toml::Value = "[sync]\nagent_url = \"http://x:1\"\n"
            .parse()
            .expect("parse");
        assert!(unset_dotpath(&mut v, "sync.agent_url"));
        assert!(get_dotpath(&v, "sync.agent_url").is_none());
    }

    /// unset 不存在的键返回 false, 不 panic.
    #[test]
    fn unset_missing_key_returns_false() {
        let mut v: toml::Value = toml::Value::Table(toml::map::Map::new());
        assert!(!unset_dotpath(&mut v, "nope.nada"));
    }

    /// set 中间 table 不存在时自动建.
    #[test]
    fn set_creates_intermediate_tables() {
        let mut v: toml::Value = toml::Value::Table(toml::map::Map::new());
        set_dotpath(&mut v, "a.b.c", toml::Value::Integer(42)).expect("set");
        let got = get_dotpath(&v, "a.b.c").expect("get");
        assert_eq!(got.as_integer(), Some(42));
    }

    /// set sync.agent_url 校验 URL 前缀.
    #[test]
    fn set_sync_url_rejects_non_http() {
        let err = set_key("sync.agent_url", "ftp://no").unwrap_err();
        assert!(format!("{err}").contains("http://"), "msg: {err}");
    }
}
