//! `frank orchestrator` 子命令 — P6 Milestone 1 真接 LocalCliWorker。
//!
//! P6 Milestone 1 (本子命令实现): 验证 LocalCliWorker 真能调本机 claude / codex /
//! opencode / gemini CLI, 走 subprocess + stdin/stdout (低 token: 不传 chat history),
//! 子进程级隔离 (多任务不串).
//!
//! 后续 Milestone:
//! - M2: axum `/orchestrator/*` REST + WebSocket + 静态 SPA (Web 可视化)
//! - M3: 多 step Job (DAG) + frank-memory 缓存 prompt 复用 (低 token 进阶)

use anyhow::Result;
use clap::{Parser, Subcommand};
use frank_orchestrator::worker::local::{CliProvider, LocalCliWorker};
use frank_orchestrator::worker::{LogLine, Worker};
use frank_orchestrator::{Job, JobId, Step, StepId, StepKind, StepStatus};

/// `frank orchestrator` 参数。
#[derive(Parser, Debug)]
pub struct Args {
    /// 子命令。
    #[command(subcommand)]
    pub command: OrchestratorCommand,
}

/// `frank orchestrator` 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum OrchestratorCommand {
    /// 真测: 调本机某个 AI CLI 跑一句 prompt, 看输出 + log 流。
    Demo(DemoArgs),
    /// 健康检查: 看本机装了哪些 CLI provider。
    Providers,
    /// (P6 M2) 启动本机 daemon + Web UI: 浏览器多 Job 看板 + WebSocket 实时 log。
    Serve(ServeArgs),
}

/// `frank orchestrator serve` 参数。
#[derive(Parser, Debug)]
pub struct ServeArgs {
    /// 监听 host:port (默认 127.0.0.1:7780, 仅本机访问保证安全)。
    #[arg(long, default_value = "127.0.0.1:7780")]
    pub bind: String,
}

/// `frank orchestrator demo` 参数。
#[derive(Parser, Debug)]
pub struct DemoArgs {
    /// AI provider: `claude` / `codex` / `opencode` / `gemini`。
    #[arg(long, default_value = "claude")]
    pub provider: String,

    /// 投递的 prompt (默认让 AI 自我介绍, 验证真跑通)。
    #[arg(long, default_value = "请用一句话告诉我你是哪个模型/CLI, 不要废话.")]
    pub prompt: String,

    /// Worker 超时 (秒, 默认 120)。
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,
}

/// 执行 orchestrator 命令。
pub fn run(args: Args) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match args.command {
            OrchestratorCommand::Demo(d) => run_demo(d).await,
            OrchestratorCommand::Providers => run_providers().await,
            OrchestratorCommand::Serve(s) => run_serve(s).await,
        }
    })
}

async fn run_serve(args: ServeArgs) -> Result<()> {
    let addr: std::net::SocketAddr = args.bind.parse()?;
    crate::cli::orchestrator_server::serve(addr).await
}

async fn run_demo(args: DemoArgs) -> Result<()> {
    let provider = parse_provider(&args.provider)?;
    let worker = LocalCliWorker::new(provider).with_timeout(args.timeout);

    if !worker.health().await {
        anyhow::bail!(
            "`{}` CLI 没装 / 不在 PATH; 跑 `which {}` 自查",
            args.provider,
            args.provider
        );
    }

    // 用 mpsc channel 接 worker 日志 (P6 后续 M2 这条 channel 接 WebSocket 给 UI)
    let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<LogLine>(64);

    // 起一个并行 task 实时打印日志 (M2 改成 WS push)
    let log_task = tokio::spawn(async move {
        while let Some(line) = log_rx.recv().await {
            let prefix = match line.level {
                frank_orchestrator::worker::LogLevel::Error => "ERR ",
                frank_orchestrator::worker::LogLevel::Warn => "WARN",
                frank_orchestrator::worker::LogLevel::Info => "INFO",
                _ => "    ",
            };
            crate::log::ui::info(&format!("[{prefix}] {}", line.message));
        }
    });

    let step = Step {
        id: StepId::new(),
        kind: StepKind::Custom("demo".to_string()),
        provider: args.provider.clone(),
        prompt: args.prompt,
        status: StepStatus::Running,
        output: None,
        started_at: Some(chrono::Utc::now()),
        completed_at: None,
    };

    crate::log::ui::section(&format!(
        "frank orchestrator demo — {} (timeout {}s)",
        args.provider, args.timeout
    ));

    let start = std::time::Instant::now();
    let result = worker.run(&step, log_tx).await;
    let elapsed = start.elapsed();

    // 关 channel 让 log_task 退出
    log_task.abort();

    match result {
        Ok(output) => {
            crate::log::ui::success(&format!(
                "`{}` 完成 ({:.1}s)",
                args.provider,
                elapsed.as_secs_f64()
            ));
            println!("\n────── stdout ──────");
            println!("{}", output.stdout.trim_end());
            println!("────────────────────\n");
        }
        Err(e) => {
            anyhow::bail!("worker run failed: {e:#}");
        }
    }

    // Job 包装 — 真测确认 P6 的 Job 抽象能用 (M2 接 REST 后这 Job 会写 JobStore)
    let _job = Job {
        id: JobId::new(),
        title: format!("demo:{}", args.provider),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        status: frank_orchestrator::JobStatus::Done,
        steps: vec![],
        workspace_path: std::env::current_dir().unwrap_or_default(),
        memory_scope: serde_json::Value::Null,
    };

    Ok(())
}

async fn run_providers() -> Result<()> {
    crate::log::ui::section("Local CLI providers (Worker health check)");
    let providers = [
        ("claude", CliProvider::Claude),
        ("codex", CliProvider::Codex),
        ("opencode", CliProvider::Opencode),
        ("gemini", CliProvider::Gemini),
    ];

    for (name, p) in providers {
        let worker = LocalCliWorker::new(p);
        let installed = worker.health().await;
        let mark = if installed { "✓" } else { "✗" };
        let detail = if installed {
            which::which(name).map_or_else(
                |_| "(found but path unresolved)".to_string(),
                |path| path.display().to_string(),
            )
        } else {
            "not in PATH".to_string()
        };
        println!("  {mark} {name:<10} {detail}");
    }
    Ok(())
}

fn parse_provider(s: &str) -> Result<CliProvider> {
    match s.to_lowercase().as_str() {
        "claude" => Ok(CliProvider::Claude),
        "codex" => Ok(CliProvider::Codex),
        "opencode" => Ok(CliProvider::Opencode),
        "gemini" => Ok(CliProvider::Gemini),
        other => {
            anyhow::bail!("unknown provider `{other}`; 支持 claude / codex / opencode / gemini")
        }
    }
}
