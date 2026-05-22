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
    ///
    /// 用每家 CLI 默认非交互 flag, 不传 system prompt / tools / model override.
    /// 模型由该 CLI 自身配置决定 (claude opus / codex gpt-5.5 / opencode qwen ...).
    Ask(AskArgs),
}

/// `frank ai ask` 参数。
#[derive(Parser, Debug)]
pub struct AskArgs {
    /// 目标 provider (claude / codex / opencode / gemini)。
    #[arg(long)]
    pub to: String,

    /// 调用方 provider (claude / codex / ...), 可选, 用于后续记到记忆 (v1 不用)。
    #[arg(long)]
    pub from: Option<String>,

    /// 投递的 prompt。可以直接传, 或用 `--` 后跟一段长文本。
    pub prompt: Vec<String>,

    /// 超时秒数 (默认 300, codex high-reasoning 慢)。
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,
}

/// 执行 ai 命令。
pub fn run(args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match args.command {
            AiCommand::Ask(a) => run_ask(a).await,
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

    let mut cmd = Command::new(bin);
    cmd.args(&cli_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // 一问一答不打扰用户, 隐 stderr
        .kill_on_drop(true);
    strip_empty_api_keys(&mut cmd);

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
            // 把回答原样 print 到 stdout (调用方终端直接看到)
            print!("{}", buf.trim_end());
            if !buf.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        Ok(Ok(status)) => {
            anyhow::bail!(
                "`{bin}` exit {} — 看 `{bin} --help` 检查认证 (尤其 claude 需 `claude setup-token` 一次)",
                status.code().unwrap_or(-1)
            );
        }
        Ok(Err(e)) => anyhow::bail!("CLI wait failed: {e}"),
        Err(_) => anyhow::bail!("`{bin}` timed out after {}s", args.timeout),
    }
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
