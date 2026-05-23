//! `frank ai` 子命令 — AI 之间一问一答的核心入口。
//!
//! 这是 frank 的**真核心价值之一**: 用户在 claude / codex / opencode 任一平台
//! 通过 slash command `/frank:<target>` 触发 → 调本命令 → 把 prompt 转发给
//! `<target>` CLI 用其默认参数跑一次 → 把回答原样 print 到 stdout (回到调用方
//! 的终端).
//!
//! # 跟 `frank orchestrator` 子命令的区别
//!
//! - `orchestrator demo/serve` 是**多任务编排平台** (Job 队列, Web UI, 实时 log),
//!   适合长任务 + 监控.
//! - `ai ask` 是**单跳一问一答**, 同步阻塞, 不要 daemon, 不要 Job tracking,
//!   不要 log channel — 就是 stdin → CLI → stdout 一条直管.
//!
//! # 不动 CLI 参数 (用户要求)
//!
//! 用每家 CLI 的**默认非交互模式 flag** (claude --print, codex exec, ...),
//! 不传任何 system prompt / tool flag / model override — 模型选 + 模式选都
//! 让 CLI 自己定 (用户已经在 CLI 配置里选好了 opus / gpt-5.5 / qwen3.6+).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use frank_orchestrator::worker::local::CliProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

/// `frank ai` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: AiCommand,
}

/// `frank ai` 子命令。
#[derive(Subcommand, Debug)]
pub enum AiCommand {
    /// 一问一答: 把 prompt 转发给目标 AI CLI, 把回答原样 print 出来。
    Ask(AskArgs),

    /// 看 frank ai ask 历史 (默认 20 条, --all 全部)。
    History(HistoryArgs),
}

/// `frank ai ask` 参数。
#[derive(Parser, Debug)]
pub struct AskArgs {
    /// 目标 provider (claude / codex / opencode / gemini)。
    #[arg(long)]
    pub to: String,

    /// 调用方 provider (claude / codex / opencode / gemini), 用于 session 追溯。
    /// SKILL.md 让 AI 调时填: claude code 触发就传 `--from claude`.
    #[arg(long)]
    pub from: Option<String>,

    /// 调用方工作目录 (用于多 session 不串). SKILL.md 让 AI 调时填 `--source-cwd "$PWD"`.
    #[arg(long)]
    pub source_cwd: Option<String>,

    /// 调用方自定义 tag (用户给的 session 标签, 可选).
    #[arg(long)]
    pub source_tag: Option<String>,

    /// 投递的 prompt。可以直接传, 或用 `--` 后跟一段长文本。
    pub prompt: Vec<String>,

    /// 超时秒数 (默认 300, codex high-reasoning 慢)。
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,
}

/// `frank ai history` 参数。
#[derive(Parser, Debug)]
pub struct HistoryArgs {
    /// 显示条数, 默认 20.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// 显示全部 (忽略 --limit).
    #[arg(long)]
    pub all: bool,

    /// 只看某个 source platform 的历史 (例 `--from claude`).
    #[arg(long)]
    pub from: Option<String>,

    /// 只看某个 source cwd (按 contains 子串匹配).
    #[arg(long)]
    pub cwd: Option<String>,
}

/// 执行 ai 命令。
pub fn run(args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match args.command {
            AiCommand::Ask(a) => run_ask(a).await,
            AiCommand::History(a) => run_history(a),
        }
    })
}

async fn run_ask(args: AskArgs) -> Result<()> {
    if args.prompt.is_empty() {
        anyhow::bail!("missing prompt (用法: `frank ai ask --to codex \"你的问题\"`)");
    }
    let prompt = args.prompt.join(" ");
    let provider = parse_provider(&args.to)?;
    let (bin, cli_args) = invocation(provider);

    if which::which(bin).is_err() {
        anyhow::bail!("`{bin}` CLI 没装 / 不在 PATH; 装好再试");
    }

    let started = std::time::Instant::now();

    let mut cmd = Command::new(bin);
    cmd.args(&cli_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // 一问一答不打扰用户, 隐 stderr
        .kill_on_drop(true);
    strip_empty_api_keys(&mut cmd);
    frank_orchestrator::worker::local::apply_proxy_config(&mut cmd);

    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn `{bin}` failed"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .context("write prompt to CLI stdin")?;
        drop(stdin);
    }

    let mut stdout = child.stdout.take().context("take CLI stdout")?;
    let mut buf = String::new();

    // 等子进程 + 超时
    match timeout(Duration::from_secs(args.timeout), async {
        let _ = stdout.read_to_string(&mut buf).await;
        child.wait().await
    })
    .await
    {
        Ok(Ok(status)) if status.success() => {
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = append_history(&HistoryEntry::ok(
                &args, &prompt, buf.trim_end(), latency_ms,
            ));
            // 把回答原样 print 到 stdout (调用方终端直接看到)
            print!("{}", buf.trim_end());
            if !buf.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        Ok(Ok(status)) => {
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let _ = append_history(&HistoryEntry::err(
                &args,
                &prompt,
                &format!("exit {}", status.code().unwrap_or(-1)),
                latency_ms,
            ));
            anyhow::bail!(
                "`{bin}` exit {} — 看 `{bin} --help` 检查认证 (尤其 claude 需 `claude setup-token` 一次)",
                status.code().unwrap_or(-1)
            );
        }
        Ok(Err(e)) => {
            let _ = append_history(&HistoryEntry::err(&args, &prompt, &format!("{e}"), 0));
            anyhow::bail!("CLI wait failed: {e}");
        }
        Err(_) => {
            let _ = append_history(&HistoryEntry::err(&args, &prompt, "timeout", 0));
            anyhow::bail!("`{bin}` timed out after {}s", args.timeout);
        }
    }
}

// ─── history 持久化 (v0.6 新, ~/.frank/ai_history.jsonl) ───────────────────

/// 一条 ai ask history 记录 (JSONL, 一行一条).
#[derive(serde::Serialize, serde::Deserialize)]
struct HistoryEntry {
    /// ISO-8601 UTC timestamp.
    ts: String,
    /// 调用方 provider (claude / codex / cli / unknown).
    from: String,
    /// 目标 provider.
    to: String,
    /// 调用方工作目录 (project 追溯关键).
    #[serde(skip_serializing_if = "Option::is_none")]
    source_cwd: Option<String>,
    /// 用户自定义 tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_tag: Option<String>,
    /// prompt 前 200 字符 (history 不存全文, 避免长 prompt 撑爆文件).
    prompt_excerpt: String,
    /// 响应前 200 字符.
    response_excerpt: String,
    /// 状态: "ok" / "err".
    status: String,
    /// 错误信息 (status=err 时).
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// 耗时 (毫秒).
    latency_ms: u64,
}

impl HistoryEntry {
    fn base(args: &AskArgs, prompt: &str) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            from: args.from.clone().unwrap_or_else(|| "unknown".to_string()),
            to: args.to.clone(),
            source_cwd: args.source_cwd.clone(),
            source_tag: args.source_tag.clone(),
            prompt_excerpt: prompt.chars().take(200).collect(),
            response_excerpt: String::new(),
            status: "ok".to_string(),
            error: None,
            latency_ms: 0,
        }
    }
    fn ok(args: &AskArgs, prompt: &str, response: &str, latency_ms: u64) -> Self {
        let mut e = Self::base(args, prompt);
        e.response_excerpt = response.chars().take(200).collect();
        e.latency_ms = latency_ms;
        e
    }
    fn err(args: &AskArgs, prompt: &str, error: &str, latency_ms: u64) -> Self {
        let mut e = Self::base(args, prompt);
        e.status = "err".to_string();
        e.error = Some(error.to_string());
        e.latency_ms = latency_ms;
        e
    }
}

fn history_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".frank").join("ai_history.jsonl"))
}

fn append_history(entry: &HistoryEntry) -> Result<()> {
    let Some(path) = history_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let line = serde_json::to_string(entry).context("serialize history entry")?;
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(f, "{line}").context("write history line")?;
    Ok(())
}

fn run_history(args: HistoryArgs) -> Result<()> {
    let Some(path) = history_path() else {
        crate::log::ui::warn("找不到 home dir, 无 history");
        return Ok(());
    };
    if !path.exists() {
        crate::log::ui::info("还没有 ai ask 历史 (跑 `frank ai ask --to <p> '...'` 第一次)");
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut entries: Vec<HistoryEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse(); // 最新在前
    if let Some(f) = &args.from {
        entries.retain(|e| &e.from == f);
    }
    if let Some(c) = &args.cwd {
        entries.retain(|e| e.source_cwd.as_deref().is_some_and(|s| s.contains(c)));
    }
    let take = if args.all {
        entries.len()
    } else {
        args.limit.min(entries.len())
    };
    crate::log::ui::section(&format!(
        "ai ask history ({} 条 / 共 {} 条)",
        take,
        entries.len()
    ));
    for e in entries.iter().take(take) {
        let cwd = e.source_cwd.as_deref().unwrap_or("");
        let tag = e
            .source_tag
            .as_deref()
            .map(|t| format!(" #{t}"))
            .unwrap_or_default();
        let status_icon = if e.status == "ok" { "✓" } else { "✗" };
        #[allow(clippy::cast_precision_loss)] // latency_ms 大于 2^53 才丢精度, 这里上限 timeout=3600s
        let latency_s = e.latency_ms as f64 / 1000.0;
        println!(
            "{} {} {} → {} ({:.1}s){}",
            status_icon,
            e.ts.split('T').next().unwrap_or(&e.ts),
            e.from,
            e.to,
            latency_s,
            tag,
        );
        println!("  cwd: {cwd}");
        println!("  Q: {}", e.prompt_excerpt);
        if e.status == "ok" {
            println!("  A: {}", e.response_excerpt);
        } else if let Some(err) = &e.error {
            println!("  ✗ {err}");
        }
        println!();
    }
    Ok(())
}

fn parse_provider(s: &str) -> Result<CliProvider> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(CliProvider::Claude),
        // gpt 是 codex 的别名 (用户原话 "/frank:gpt" 想触发 codex)
        "codex" | "gpt" | "openai" => Ok(CliProvider::Codex),
        "opencode" | "qwen" => Ok(CliProvider::Opencode),
        "gemini" | "google" => Ok(CliProvider::Gemini),
        other => anyhow::bail!(
            "unknown target `{other}`; 支持: claude / codex(gpt) / opencode(qwen) / gemini"
        ),
    }
}

/// 每家 CLI 的"非交互一问一答"调用方式。
///
/// 注: 这里**只用** 进入非交互模式必需的最小 flag, 不传任何 system prompt / tools /
/// model override — 用户在 CLI 配置里选好的模型 / 模式都保持原样.
fn invocation(p: CliProvider) -> (&'static str, Vec<&'static str>) {
    match p {
        // claude --print 走非交互, 从 stdin 读 prompt (或 --print 后跟 arg, 我们走 stdin)
        CliProvider::Claude => ("claude", vec!["--print"]),
        // codex exec - 从 stdin 读
        CliProvider::Codex => ("codex", vec!["exec", "--skip-git-repo-check", "-"]),
        // opencode run - 从 stdin 读 (opencode 0.x 文档)
        CliProvider::Opencode => ("opencode", vec!["run", "-"]),
        // gemini --prompt -
        CliProvider::Gemini => ("gemini", vec!["--prompt", "-"]),
    }
}

/// 跟 LocalCliWorker 同款: 清空字符串 API key env, 避免 401 陷阱。
fn strip_empty_api_keys(cmd: &mut Command) {
    const SUSPECT: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
    ];
    for key in SUSPECT {
        if std::env::var(key).is_ok_and(|v| v.trim().is_empty()) {
            cmd.env_remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_aliases() {
        assert!(matches!(parse_provider("gpt").unwrap(), CliProvider::Codex));
        assert!(matches!(
            parse_provider("qwen").unwrap(),
            CliProvider::Opencode
        ));
        assert!(matches!(
            parse_provider("Claude").unwrap(),
            CliProvider::Claude
        ));
        assert!(parse_provider("unknown").is_err());
    }

    #[test]
    fn invocation_uses_minimal_flags() {
        let (bin, args) = invocation(CliProvider::Claude);
        assert_eq!(bin, "claude");
        // 只 --print, 不带 system prompt / tools (用户要求 "用原始参数")
        assert_eq!(args, vec!["--print"]);
    }
}
