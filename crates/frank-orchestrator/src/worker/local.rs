//! Local CLI worker — 把本机已装的 AI CLI (claude / codex / opencode / gemini ...)
//! 作为 subprocess 包起来, 通过 stdin/stdout 投递 prompt + 捕获输出。
//!
//! # 解决的痛点
//!
//! - 用户买了 Claude opus / codex 5.5 plus / opencode go 套餐, 想本地 CLI 协作
//! - CCB 走 tmux pane keypress 模拟, 慢且不可靠;
//!   这里直接 spawn subprocess + 喂 stdin, OS 级隔离, 多 Job 互不干扰 (各自子进程)
//!
//! # 进程隔离 (多任务不串)
//!
//! 每个 [`LocalCliWorker`] 实例**只在一次 `run()` 里活**, 启一个 subprocess、塞 prompt、
//! 读 stdout 到 EOF (或超时)、杀进程、返回. Job-A 的 worker 和 Job-B 的 worker
//! 各自独立 subprocess, **天然不串** (OS pid 隔离, 不共享 tmux session).
//!
//! 多 step 同 job 的中间状态由 [`crate::Executor`] 通过 `StepOutput.structured`
//! 上下文传递 (低 token: 只传 diff / 结果, 不传整个 chat history).
//!
//! # 支持的 CLI
//!
//! - `claude` (Anthropic Claude Code CLI) — `claude --print <prompt>` 非交互模式
//! - `codex` (OpenAI Codex CLI) — `codex exec --skip-git-repo-check <prompt>`
//! - `opencode` (open-source) — `opencode run <prompt>` (按 opencode 0.x 文档)
//! - `gemini` (Google Gemini CLI) — `gemini --prompt <prompt>`
//!
//! 每家 CLI 调用法略不同, 通过 [`CliProvider`] 枚举封装具体 flag.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::job::{Step, StepOutput};
use crate::worker::{LogLevel, LogLine, Worker, WorkerId};

/// 支持的本地 CLI provider 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliProvider {
    /// Anthropic Claude Code CLI (`claude`)。
    Claude,
    /// OpenAI Codex CLI (`codex`)。
    Codex,
    /// open-source `opencode` CLI。
    Opencode,
    /// Google Gemini CLI (`gemini`)。
    Gemini,
}

impl CliProvider {
    fn bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    /// 默认调用参数 (非交互 exec 模式)。prompt 通过 stdin 投递, 避免 shell 转义 + 太长 arg 风险。
    fn args(self) -> Vec<&'static str> {
        match self {
            Self::Claude => vec!["--print"],
            Self::Codex => vec!["exec", "--skip-git-repo-check", "-"],
            Self::Opencode => vec!["run", "-"],
            Self::Gemini => vec!["--prompt", "-"],
        }
    }
}

/// Local CLI worker。
///
/// 构造时指定 provider; 每次 `run()` 起一个新 subprocess (天然隔离)。
pub struct LocalCliWorker {
    id: WorkerId,
    provider: CliProvider,
    /// 子进程超时 (默认 300s, 防 CLI hang 导致 Job 卡死)。
    timeout: Duration,
    /// 可选 working directory (Job 隔离 workspace)。
    workspace: Option<std::path::PathBuf>,
}

impl LocalCliWorker {
    /// 用 provider 构造。worker id 自动为 provider 字符串。
    #[must_use]
    pub fn new(provider: CliProvider) -> Self {
        Self {
            id: WorkerId::new(provider.bin()),
            provider,
            timeout: Duration::from_secs(300),
            workspace: None,
        }
    }

    /// 设 step 工作目录 (Executor 给每个 Job 一个独立 dir)。
    #[must_use]
    pub fn with_workspace(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.workspace = Some(dir.into());
        self
    }

    /// 调整超时 (默认 300s)。
    #[must_use]
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = Duration::from_secs(secs);
        self
    }
}

#[async_trait]
impl Worker for LocalCliWorker {
    fn id(&self) -> &WorkerId {
        &self.id
    }

    async fn health(&self) -> bool {
        // 仅看 binary 在 PATH 里 (跑 --version 也行但更慢)
        which::which(self.provider.bin()).is_ok()
    }

    #[allow(clippy::too_many_lines)] // 分支 (prompt arg vs stdin) + IO + 日志 + timeout 都在 run 里, 拆函数会重复 borrow
    async fn run(&self, step: &Step, log_tx: mpsc::Sender<LogLine>) -> Result<StepOutput> {
        let bin = self.provider.bin();
        let args = self.provider.args();
        // claude --print 实测: 新版要求 prompt 必须作为 argument (stdin pipe EOF 拿不到).
        // 其他 CLI (codex/opencode/gemini) 用 `-` 占位符接 stdin, 仍走 pipe.
        let prompt_via_arg = matches!(self.provider, CliProvider::Claude);

        let _ = log_tx
            .send(LogLine::info(format!(
                "spawn `{} {}` (workspace={:?}, timeout={}s)",
                bin,
                args.join(" "),
                self.workspace.as_deref().map(std::path::Path::display),
                self.timeout.as_secs()
            )))
            .await;

        let mut cmd = Command::new(bin);
        cmd.args(&args);
        if prompt_via_arg {
            cmd.arg(&step.prompt);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        strip_empty_api_keys(&mut cmd);
        if let Some(ws) = &self.workspace {
            cmd.current_dir(ws);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn `{bin}` (是否安装 + PATH 里?)"))?;

        // 把 prompt 塞 stdin (claude 已通过 arg 拿到了, 这里跳过, 直接 close stdin 让 CLI 不卡等输入)
        if let Some(mut stdin) = child.stdin.take() {
            if !prompt_via_arg {
                stdin
                    .write_all(step.prompt.as_bytes())
                    .await
                    .context("write prompt to subprocess stdin")?;
            }
            // shutdown() 显式刷缓冲 + close fd, 触发 EOF.
            // drop 在 tokio runtime 下不保证立即 close, 必须 shutdown.
            stdin
                .shutdown()
                .await
                .context("shutdown subprocess stdin")?;
        }

        let stdout = child.stdout.take().context("take subprocess stdout")?;
        let stderr = child.stderr.take().context("take subprocess stderr")?;

        // 并行读 stdout (主响应) + stderr (按行 stream 到 log_tx)
        let log_tx_err = log_tx.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = log_tx_err
                    .send(LogLine::new(LogLevel::Debug, format!("[stderr] {line}")))
                    .await;
            }
        });

        // stdout 全收 (是主输出 — 给 StepOutput.stdout)
        let stdout_task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut reader = BufReader::new(stdout);
            let _ = reader.read_to_string(&mut buf).await;
            buf
        });

        // 等子进程 + 超时
        let status = match timeout(self.timeout, child.wait()).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(anyhow!("子进程 wait 失败: {e}"));
            }
            Err(_) => {
                // 超时 → kill
                let _ = log_tx
                    .send(LogLine::warn(format!(
                        "{bin} 超时 ({}s), kill",
                        self.timeout.as_secs()
                    )))
                    .await;
                // child 在 .wait() 拿走了 ownership; kill_on_drop 已设, 这里直接 return
                return Err(anyhow!("local CLI `{bin}` timed out"));
            }
        };

        let stdout_str = stdout_task.await.unwrap_or_default();
        let _ = stderr_task.await;

        let _ = log_tx
            .send(LogLine::info(format!(
                "`{bin}` exit code={} (stdout {} bytes)",
                status.code().unwrap_or(-1),
                stdout_str.len()
            )))
            .await;

        if !status.success() {
            // 非 0 退出: 把 stdout 摘要也带上, 用户能立刻看到 CLI 的真错误信息
            // (例: claude 401 / codex network error 等都打 stdout, 不带就只看到 exit code 没法诊断)
            let preview: String = stdout_str.chars().take(400).collect();
            let hint = if bin == "claude" && preview.contains("authentication") {
                "\n💡 修复: 跑 `claude setup-token` 一次登录 CLI (Pro 订阅 OAuth)"
            } else if preview.contains("401") || preview.contains("Unauthorized") {
                "\n💡 修复: 检查该 CLI 的 auth 配置, 跑 `<bin> auth login` 或 setup-token"
            } else {
                ""
            };
            return Err(anyhow!(
                "local CLI `{bin}` exit {} — output:\n{}{hint}",
                status.code().unwrap_or(-1),
                preview
            ));
        }

        Ok(StepOutput {
            stdout: stdout_str,
            structured: serde_json::json!({
                "provider": bin,
                "exit_code": status.code(),
            }),
        })
    }
}

/// 清理"空字符串 API key"陷阱.
///
/// Claude Code 桌面 app / 某些 IDE 启动时把空 `ANTHROPIC_API_KEY=""` 注入 shell env;
/// claude / codex / gemini CLI 检测到 env 存在 (即便空) 就走 "API key 认证" 路径,
/// 用空字符串调 API → 401. 子进程级 env_remove 让 CLI 看不到这些变量, 自动回退
/// OAuth/keychain (Pro/Plus/Go 订阅真路径).
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
            tracing::debug!("unset empty {key} from subprocess env (avoid 401 trap)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_bin_names() {
        assert_eq!(CliProvider::Claude.bin(), "claude");
        assert_eq!(CliProvider::Codex.bin(), "codex");
        assert_eq!(CliProvider::Opencode.bin(), "opencode");
        assert_eq!(CliProvider::Gemini.bin(), "gemini");
    }

    #[test]
    fn worker_id_matches_bin() {
        let w = LocalCliWorker::new(CliProvider::Codex);
        assert_eq!(w.id().as_str(), "codex");
    }

    #[test]
    fn timeout_default_is_300s() {
        let w = LocalCliWorker::new(CliProvider::Claude);
        assert_eq!(w.timeout, Duration::from_secs(300));
    }

    #[test]
    fn timeout_override_works() {
        let w = LocalCliWorker::new(CliProvider::Claude).with_timeout(60);
        assert_eq!(w.timeout, Duration::from_secs(60));
    }
}
