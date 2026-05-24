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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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

impl From<CliProvider> for frank_cred::Provider {
    fn from(p: CliProvider) -> Self {
        match p {
            CliProvider::Claude => Self::Claude,
            CliProvider::Codex => Self::Codex,
            CliProvider::Opencode => Self::Opencode,
            CliProvider::Gemini => Self::Gemini,
        }
    }
}

// tokio Command 的 CommandEnv impl 在 frank-cred 自家 (feature = "tokio-cmd"),
// 因 orphan rule frank-orchestrator 不能在这里 impl。

impl CliProvider {
    fn bin(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Gemini => "gemini",
        }
    }

    /// 按各家 `--help` 官方语法把 prompt 直接拼成 positional/flag arg。
    ///
    /// - `claude [--model <m>] --print <prompt>`
    /// - `codex exec --skip-git-repo-check [--model <m>] <prompt>`
    /// - `opencode run [--model <m>] <message...>`
    /// - `gemini [--model <m>] --prompt <prompt>`
    ///
    /// 之前版本用 `-` 占位走 stdin pipe — 各家行为不一致 (opencode 把 `-` 当字面 message),
    /// 且 tokio drop(stdin) 不可靠. 改成统一走 arg 后, stdin 全置 Stdio::null().
    fn args(self, prompt: &str, model: Option<&str>) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        match self {
            Self::Claude => {
                args.push("--print".into());
                if let Some(m) = model {
                    args.push("--model".into());
                    args.push(m.into());
                }
                args.push(prompt.into());
            }
            Self::Codex => {
                args.push("exec".into());
                args.push("--skip-git-repo-check".into());
                if let Some(m) = model {
                    args.push("--model".into());
                    args.push(m.into());
                }
                args.push(prompt.into());
            }
            Self::Opencode => {
                args.push("run".into());
                if let Some(m) = model {
                    args.push("--model".into());
                    args.push(m.into());
                }
                args.push(prompt.into());
            }
            Self::Gemini => {
                if let Some(m) = model {
                    args.push("--model".into());
                    args.push(m.into());
                }
                args.push("--prompt".into());
                args.push(prompt.into());
            }
        }
        args
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
    /// 可选 model (传给 CLI 的 --model 参数)。空时各家 CLI 用自家默认。
    model: Option<String>,
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
            model: None,
        }
    }

    /// 设 model (例 "opus" / "gpt-5.5" / "mimo-v2.5-pro")。空 = 用 CLI 自家默认。
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
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

    async fn run(&self, step: &Step, log_tx: mpsc::Sender<LogLine>) -> Result<StepOutput> {
        let bin = self.provider.bin();
        // 所有 4 家 CLI 官方语法都接 prompt 作 positional/flag arg, 不再走 stdin.
        let args = self.provider.args(&step.prompt, self.model.as_deref());

        let _ = log_tx
            .send(LogLine::info(format!(
                "spawn `{bin}` with {} args (workspace={:?}, timeout={}s)",
                args.len(),
                self.workspace.as_deref().map(std::path::Path::display),
                self.timeout.as_secs()
            )))
            .await;

        let mut cmd = Command::new(bin);
        cmd.args(&args)
            .stdin(Stdio::null()) // 不喂 stdin — prompt 已是 arg, 子进程别等输入
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // v0.10.4 ADR-009: 5 层 fallback 找凭据 → 注 env (跨进程链解决 Keychain ACL).
        // 命中: 注入 env var (claude --print 等优先读 env, 绕开 ACL).
        // miss: strip_empty_api_keys 兜底清空值, 让 CLI 自家逻辑跑.
        let cred_report = resolve_and_inject_or_strip(&mut cmd, self.provider);
        if let Some(report) = &cred_report {
            let _ = log_tx
                .send(LogLine::info(format!(
                    "frank-cred ✓ {} 注入 {} (source: {})",
                    if report.injected_env { "env" } else { "file" },
                    report.env_var.as_deref().unwrap_or("-"),
                    report.source
                )))
                .await;
        }
        apply_proxy_config(&mut cmd);
        if let Some(ws) = &self.workspace {
            cmd.current_dir(ws);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn `{bin}` (是否安装 + PATH 里?)"))?;

        let stdout = child.stdout.take().context("take subprocess stdout")?;
        let stderr = child.stderr.take().context("take subprocess stderr")?;

        // 并行读 stdout (主响应) + stderr (按行 stream 到 log_tx).
        // v0.10.4 ADR-009 M2: 每条 line / stdout buf 经 frank_cred::redact_secrets 屏蔽 token,
        // 防止 child CLI echo token 到 WS / Web UI 时泄漏.
        let log_tx_err = log_tx.clone();
        let stderr_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let safe = frank_cred::redact_secrets(&line);
                let _ = log_tx_err
                    .send(LogLine::new(LogLevel::Debug, format!("[stderr] {safe}")))
                    .await;
            }
        });

        // stdout 全收 (是主输出 — 给 StepOutput.stdout). redact 后才放 StepOutput.
        let stdout_task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut reader = BufReader::new(stdout);
            let _ = reader.read_to_string(&mut buf).await;
            // V3 M2: redact 全 stdout, 防 WS frame / Web UI 泄漏
            frank_cred::redact_secrets(&buf)
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
                "\n💡 修复 (v0.10.4): 跑 `frank login provider claude` (自动 wrap setup-token)"
            } else if preview.contains("401") || preview.contains("Unauthorized") {
                "\n💡 修复 (v0.10.4): `frank login provider <claude|codex|gemini|opencode>` (ADR-009 凭据桥)"
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

/// v0.10.4 ADR-009: 跨进程 CLI 凭据桥的主入口.
///
/// 流程:
/// 1. 试 [`frank_cred::resolve_and_inject`] — 5 层 fallback (env/frank-store/official/keyring/security CLI).
///    命中 → 注入 env var (按 TokenKind 决定策略, V3 实施: 三类都注 env).
/// 2. miss 或 frank-cred 解析失败 → 退回 [`strip_empty_api_keys`] 兜底 (清空值, CLI 自家逻辑).
///
/// 返回 `Some(InjectReport)` 表示命中, `None` 表示退回兜底.
#[must_use]
pub fn resolve_and_inject_or_strip(
    cmd: &mut Command,
    provider: CliProvider,
) -> Option<frank_cred::InjectReport> {
    match frank_cred::resolve_and_inject(cmd, provider.into()) {
        Ok(report) => Some(report),
        Err(e) => {
            tracing::debug!("frank-cred miss ({provider:?}): {e}; fallback strip_empty");
            strip_empty_api_keys(cmd);
            None
        }
    }
}

/// 清理"空字符串 API key"陷阱 (兜底, 当 frank-cred 5 层全 miss 时).
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

/// 从 `~/.frank/config.toml` 读 `[proxy]` 配置并 inject 给子进程 env。
///
/// 修 v0.6.1 真问题: brew services 启的 daemon **不读 ~/.zshrc** 也**不继承用户 shell**
/// 的 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY env, 导致 spawn 的 claude/codex 直连 openai.com /
/// anthropic.com 走默认网络 → 国内 Clash 用户疯狂 reconnect.
///
/// config.toml 格式:
/// ```toml
/// [proxy]
/// http  = "http://127.0.0.1:7897"
/// https = "http://127.0.0.1:7897"
/// all   = "http://127.0.0.1:7897"
/// no    = "localhost,127.0.0.1,::1,.local"
/// ```
///
/// 缺字段就不 inject (用户机器没配 proxy 时安全不动 env).
pub fn apply_proxy_config(cmd: &mut Command) {
    let Some(home) = dirs::home_dir() else { return };
    let path = home.join(".frank").join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(v) = text.parse::<toml::Value>() else {
        return;
    };
    let Some(proxy) = v.get("proxy").and_then(toml::Value::as_table) else {
        return;
    };
    // 同时设大写 + 小写两套, 兼容各家 CLI 习惯 (golang 程序看小写, python 程序看大写)
    for (toml_key, env_keys) in [
        ("http", &["HTTP_PROXY", "http_proxy"][..]),
        ("https", &["HTTPS_PROXY", "https_proxy"][..]),
        ("all", &["ALL_PROXY", "all_proxy"][..]),
        ("no", &["NO_PROXY", "no_proxy"][..]),
    ] {
        if let Some(val) = proxy.get(toml_key).and_then(toml::Value::as_str) {
            for env_key in env_keys {
                cmd.env(env_key, val);
            }
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
