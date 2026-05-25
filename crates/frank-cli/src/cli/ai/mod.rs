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

pub mod history_store;
pub mod models;
pub mod skill_gen;
pub mod sources;

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand};
use frank_orchestrator::worker::local::CliProvider;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::timeout;

use history_store::{HistoryEntry, HistoryStore, ListFilter};

/// 帮 `run_ask` 拼一条 history 摘要 (成功路径).
///
/// 调用方拿到这个 entry 后再交给 `HistoryStore::append` 写到索引 + 全文文件.
fn entry_ok(args: &AskArgs, prompt: &str, response: &str, latency_ms: u64) -> HistoryEntry {
    HistoryEntry {
        id: HistoryStore::new_id(),
        ts: Utc::now().to_rfc3339(),
        from: args.from.clone().unwrap_or_else(|| "unknown".to_string()),
        to: args.to.clone(),
        source_cwd: args.source_cwd.clone(),
        source_tag: args.source_tag.clone(),
        model: args.model.clone(),
        prompt_excerpt: prompt.chars().take(200).collect(),
        response_excerpt: response.chars().take(200).collect(),
        status: "ok".to_string(),
        error: None,
        latency_ms,
    }
}

/// 帮 `run_ask` 拼一条 history 摘要 (失败路径).
fn entry_err(args: &AskArgs, prompt: &str, error: &str, latency_ms: u64) -> HistoryEntry {
    HistoryEntry {
        id: HistoryStore::new_id(),
        ts: Utc::now().to_rfc3339(),
        from: args.from.clone().unwrap_or_else(|| "unknown".to_string()),
        to: args.to.clone(),
        source_cwd: args.source_cwd.clone(),
        source_tag: args.source_tag.clone(),
        model: args.model.clone(),
        prompt_excerpt: prompt.chars().take(200).collect(),
        response_excerpt: String::new(),
        status: "err".to_string(),
        error: Some(error.to_string()),
        latency_ms,
    }
}

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
    /// `--list-models` 模式下不需要 (仅列模型不实际 ask).
    #[arg(long, required_unless_present = "list_models", default_value = "")]
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

    /// 可选 model (例 `haiku`/`opus` for claude, `gpt-5.4-mini` for codex). 空 = CLI 默认.
    #[arg(long)]
    pub model: Option<String>,

    /// 投递的 prompt。可以直接传, 或用 `--` 后跟一段长文本。
    pub prompt: Vec<String>,

    /// 超时秒数 (默认 300, codex high-reasoning 慢)。
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,

    /// v0.8: 共享上下文注入 — 调 cli 前从 frank-memory 搜 top-3 相关记忆注入 prompt 前缀.
    /// scope 默认 `default`, 多设备/多 session 用不同 tag 隔离 (`--context-from project-x`).
    /// **不指定 = 不注入**, 完全跟旧版兼容; 注入失败 (sync-agent 不可用) 也降级到不注入, 不阻塞 ask.
    #[arg(long, value_name = "SESSION_TAG")]
    pub context_from: Option<String>,

    /// v0.8: 关掉自动存 (ask 完成后异步把 prompt+response 存 frank-memory). 默认存.
    #[arg(long)]
    pub no_save: bool,

    /// v0.10.7 (D1): 列出 4 家 CLI 当前能用的模型, 不实际跑 ask.
    ///
    /// 用法 `frank ai ask --list-models` (prompt 可空). 输出 claude/codex/opencode/gemini
    /// 各自支持的 model 名 (内置清单 + opencode 实时拉 + `~/.frank/models.yaml` 用户自定义).
    #[arg(long)]
    pub list_models: bool,
}

/// `frank ai history` 参数。
///
/// v0.10.7 D5: 拆成子命令 list / show / delete / export。
/// 不传子命令 = 沿用老行为, 等价于 `list`。
#[derive(Parser, Debug)]
pub struct HistoryArgs {
    /// 子命令 (`list` / `show` / `delete` / `export`). 不传 = list.
    #[command(subcommand)]
    pub command: Option<HistoryCmd>,
}

/// `frank ai history` 子命令。
#[derive(Subcommand, Debug)]
pub enum HistoryCmd {
    /// 列历史 (摘要表格, 支持 filter)。
    List(ListArgs),
    /// 看一条的完整 prompt + 回答 (按 id)。
    Show(ShowArgs),
    /// 删: 单删 id, 或批删 `--before <日期>`。
    Delete(DeleteArgs),
    /// 全量导出 (jsonl / md)。重定向到文件: `frank ai history export > h.md`.
    Export(ExportArgs),
}

/// `frank ai history list` 参数。
#[derive(Parser, Debug)]
pub struct ListArgs {
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

    /// 只看某个目标 provider (例 `--provider codex` 只看 codex 的).
    #[arg(long)]
    pub provider: Option<String>,

    /// 只看某个状态 (`ok` / `err`).
    #[arg(long)]
    pub status: Option<String>,

    /// 只看某天之后 (YYYY-MM-DD).
    #[arg(long)]
    pub since: Option<String>,
}

/// `frank ai history show` 参数。
#[derive(Parser, Debug)]
pub struct ShowArgs {
    /// 历史短码 id (`frank ai history list` 第一列那个).
    pub id: String,
}

/// `frank ai history delete` 参数。
#[derive(Parser, Debug)]
pub struct DeleteArgs {
    /// 单删: 历史短码 id (与 --before 二选一).
    pub id: Option<String>,
    /// 批删: 此日期之前的全删 (YYYY-MM-DD).
    #[arg(long, conflicts_with = "id")]
    pub before: Option<String>,
}

/// `frank ai history export` 参数。
#[derive(Parser, Debug)]
pub struct ExportArgs {
    /// 导出格式: `jsonl` (默认) 或 `md`.
    #[arg(long, default_value = "jsonl")]
    pub format: String,
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
    // D1: --list-models 短路 — 列模型不实际 spawn CLI, prompt 可空.
    if args.list_models {
        return models::print_all();
    }
    if args.prompt.is_empty() {
        anyhow::bail!("missing prompt (用法: `frank ai ask --to codex \"你的问题\"`)");
    }
    let raw_prompt = args.prompt.join(" ");
    // v0.8 共享上下文: --context-from <tag> 触发, 失败 graceful 降级到不注入
    let prompt = inject_context_if_requested(&raw_prompt, &args).await;
    let provider = parse_provider(&args.to)?;
    let (bin, cli_args) = invocation(provider, args.model.as_deref());

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
    // v0.10.4 ADR-009: 5 层 fallback 找凭据 → 注 env. miss 退回 strip_empty 兜底.
    let cred_report =
        frank_orchestrator::worker::local::resolve_and_inject_or_strip(&mut cmd, provider);
    if let Some(r) = &cred_report {
        // stderr 一行可观测 (用户能看到走哪层 ACL 命中, 零 token 消耗)
        eprintln!(
            "[frank-cred] ✓ {} (source: {})",
            r.env_var.as_deref().unwrap_or("(no-inject)"),
            r.source
        );
    }
    // 保留 strip_empty 兜底逻辑 (resolve_and_inject_or_strip 内部已调过, 这行去掉)
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
            // v0.10.5: claude/codex 走 JSON 解析提 token+cost; gemini/opencode 直走 raw.
            let (parsed_reply, call_report) = extract_reply_and_report(
                provider,
                &buf,
                latency_ms,
                args.model.as_deref().unwrap_or(""),
            );
            let response = parsed_reply.trim_end().to_string();
            // 解析成功 → 一行 stderr 可观测 (跟 [frank-cred] 行并排)
            if let Some(r) = call_report {
                eprintln!("{}", r.render_oneline());
            }
            let entry = entry_ok(&args, &raw_prompt, &response, latency_ms);
            let _ = HistoryStore::append(&entry, &raw_prompt, &response);
            // v0.8 自动存: 异步把 (raw_prompt, response) 存 frank-memory, 失败仅 warn
            if !args.no_save {
                save_to_memory_if_possible(&args, &raw_prompt, &response).await;
            }
            // 把回答原样 print 到 stdout (调用方终端直接看到)
            print!("{response}");
            if !response.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        Ok(Ok(status)) => {
            let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let err_msg = format!("exit {}", status.code().unwrap_or(-1));
            let entry = entry_err(&args, &prompt, &err_msg, latency_ms);
            let _ = HistoryStore::append(&entry, &prompt, "");
            anyhow::bail!(
                "`{bin}` exit {} — 看 `{bin} --help` 检查认证 (尤其 claude 需 `claude setup-token` 一次)",
                status.code().unwrap_or(-1)
            );
        }
        Ok(Err(e)) => {
            let err_msg = format!("{e}");
            let entry = entry_err(&args, &prompt, &err_msg, 0);
            let _ = HistoryStore::append(&entry, &prompt, "");
            anyhow::bail!("CLI wait failed: {e}");
        }
        Err(_) => {
            let entry = entry_err(&args, &prompt, "timeout", 0);
            let _ = HistoryStore::append(&entry, &prompt, "");
            anyhow::bail!("`{bin}` timed out after {}s", args.timeout);
        }
    }
}

// ─── v0.8 共享上下文 (memory inject + auto-save) ───────────────────────────

/// 调 sync-agent search → 拼成 "## Recent Context\n...\n\n## Question\n{prompt}" 前缀.
///
/// 失败 graceful: 无 token / sync-agent 不可用 / search 返回空 → 返回原 prompt 不变.
/// 不打扰用户终端 — 只在 RUST_LOG=debug 时记录失败原因.
async fn inject_context_if_requested(raw_prompt: &str, args: &AskArgs) -> String {
    let Some(session) = args.context_from.as_deref() else {
        return raw_prompt.to_string(); // 不指定 = 不注入
    };
    let result = tokio::task::spawn_blocking({
        let session = session.to_string();
        let prompt = raw_prompt.to_string();
        move || -> anyhow::Result<Vec<String>> {
            let client = crate::sync_client::SyncClient::from_env_or_config()?;
            let scope = frank_memory::Scope {
                user_id: None,
                agent_id: None,
                session_id: Some(session),
            };
            let matches = client.search(&prompt, &scope, Some(3), Some(0.3))?;
            Ok(matches.into_iter().map(|m| m.record.content).collect())
        }
    })
    .await;
    let facts = match result {
        Ok(Ok(facts)) if !facts.is_empty() => facts,
        Ok(Ok(_)) => {
            tracing::debug!("inject_context: no relevant memory");
            return raw_prompt.to_string();
        }
        Ok(Err(e)) => {
            tracing::debug!("inject_context: search failed: {e:#}");
            return raw_prompt.to_string();
        }
        Err(e) => {
            tracing::debug!("inject_context: spawn_blocking failed: {e}");
            return raw_prompt.to_string();
        }
    };
    use std::fmt::Write as _;
    let mut s = String::from("## Recent Context (from shared memory)\n");
    for (i, f) in facts.iter().enumerate() {
        let _ = writeln!(s, "{}. {f}", i + 1);
    }
    s.push_str("\n## Question\n");
    s.push_str(raw_prompt);
    s
}

/// ask 完成后异步存 (prompt, response) 到 frank-memory.
///
/// v0.12.0 改: 默认存 (--no-save 关). --context-from 指定时用作 session_id, 否则用 "auto".
/// 不抽事实, 直接拼 "Q: ...\nA: ..." 作为 raw fact 入库 (用户原话: 简单可用先, mem0 风格抽事实留 v0.13).
/// 失败 graceful (sync-agent 不可用就跳过), 不影响 ask 返回值.
async fn save_to_memory_if_possible(args: &AskArgs, raw_prompt: &str, response: &str) {
    if args.no_save {
        return;
    }
    let session = args
        .context_from
        .clone()
        .unwrap_or_else(|| "auto".to_string());
    let fact = format!(
        "Q ({} → {}): {}\nA: {}",
        args.from.as_deref().unwrap_or("user"),
        args.to,
        raw_prompt.chars().take(500).collect::<String>(),
        response.chars().take(1500).collect::<String>(),
    );
    let metadata = serde_json::json!({
        "from": args.from.clone().unwrap_or_default(),
        "to": args.to.clone(),
        "source_cwd": args.source_cwd.clone().unwrap_or_default(),
        "source_tag": args.source_tag.clone().unwrap_or_default(),
    });
    let _ = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let client = crate::sync_client::SyncClient::from_env_or_config()?;
        let scope = frank_memory::Scope {
            user_id: None,
            agent_id: None,
            session_id: Some(session),
        };
        client.add_raw(&fact, &scope, Some(&metadata))?;
        Ok(())
    })
    .await
    .map_err(|e| {
        tracing::debug!("save_to_memory: spawn_blocking failed: {e}");
    })
    .and_then(|r| {
        r.map_err(|e| {
            tracing::debug!("save_to_memory: add_raw failed: {e:#}");
        })
    });
}

// ─── history 命令分发 (D5 — 接 history_store::HistoryStore) ──────────────────

fn run_history(args: HistoryArgs) -> Result<()> {
    let cmd = args
        .command
        .unwrap_or(HistoryCmd::List(ListArgs::default()));
    match cmd {
        HistoryCmd::List(a) => run_history_list(a),
        HistoryCmd::Show(a) => run_history_show(a),
        HistoryCmd::Delete(a) => run_history_delete(a),
        HistoryCmd::Export(a) => run_history_export(a),
    }
}

fn run_history_list(args: ListArgs) -> Result<()> {
    let filter = ListFilter {
        provider: args.provider,
        status: args.status,
        since: args.since.as_deref().and_then(parse_since),
        cwd: args.cwd,
        // 拉全表, 后面 from + limit 自己再过 / 截
        limit: None,
    };
    let mut entries = HistoryStore::list(&filter)?;
    if let Some(f) = &args.from {
        entries.retain(|e| &e.from == f);
    }
    let total = entries.len();
    let take = if args.all {
        entries.len()
    } else {
        args.limit.min(entries.len())
    };
    crate::log::ui::section(&format!("ai ask history ({take} 条 / 共 {total} 条)"));
    for e in entries.iter().take(take) {
        let cwd = e.source_cwd.as_deref().unwrap_or("");
        let tag = e
            .source_tag
            .as_deref()
            .map(|t| format!(" #{t}"))
            .unwrap_or_default();
        let status_icon = if e.status == "ok" { "✓" } else { "✗" };
        #[allow(clippy::cast_precision_loss)]
        // latency_ms 大于 2^53 才丢精度, 这里上限 timeout=3600s
        let latency_s = e.latency_ms as f64 / 1000.0;
        let model = e.model.as_deref().unwrap_or("-");
        println!(
            "{} {}  {} → {} [{}] ({:.1}s){}  id={}",
            status_icon,
            e.ts.split('T').next().unwrap_or(&e.ts),
            e.from,
            e.to,
            model,
            latency_s,
            tag,
            e.id,
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

fn run_history_show(args: ShowArgs) -> Result<()> {
    let full = HistoryStore::show(&args.id)?;
    crate::log::ui::section(&format!("history {} — {}", args.id, full.ts));
    println!("# Q\n{}\n", full.prompt);
    println!("# A\n{}", full.response);
    Ok(())
}

fn run_history_delete(args: DeleteArgs) -> Result<()> {
    match (args.id, args.before) {
        (Some(id), _) => {
            HistoryStore::delete(&id)?;
            crate::log::ui::success(&format!("删了 {id}"));
        }
        (None, Some(before)) => {
            let cutoff = parse_since(&before)
                .ok_or_else(|| anyhow::anyhow!("`--before` 要 YYYY-MM-DD 格式, 收到 `{before}`"))?;
            let n = HistoryStore::delete_before(cutoff)?;
            crate::log::ui::success(&format!("删了 {n} 条 (在 {before} 之前)"));
        }
        (None, None) => {
            anyhow::bail!("要 `frank ai history delete <id>` 或 `frank ai history delete --before YYYY-MM-DD`");
        }
    }
    Ok(())
}

fn run_history_export(args: ExportArgs) -> Result<()> {
    let out = HistoryStore::export(&args.format)?;
    // 不走 ui::* (因为用户会 `> file` 重定向 stdout)
    print!("{out}");
    Ok(())
}

/// 把 `YYYY-MM-DD` (或完整 RFC3339) parse 成 UTC `DateTime`.
///
/// 简单做法: 优先按 `YYYY-MM-DD` 配 `00:00:00 UTC`; 失败再 fallback RFC3339.
fn parse_since(s: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|naive| Utc.from_utc_datetime(&naive))
        .or_else(|| s.parse::<DateTime<Utc>>().ok())
}

impl Default for ListArgs {
    fn default() -> Self {
        Self {
            limit: 20,
            all: false,
            from: None,
            cwd: None,
            provider: None,
            status: None,
            since: None,
        }
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

/// 每家 CLI 的"非交互一问一答"调用方式. 可选 model 通过 --model 注入.
///
/// 注: 这里**只用** 进入非交互模式必需的最小 flag. 用户传 --model 时按各家官方语法注入.
///
/// v0.10.5 (Phase 1): claude/codex 额外加 JSON 输出 flag (`--output-format json` /
/// `--json`), 用于 ai_report 解析 token + cost + session. gemini/opencode 暂不动
/// (TODO v0.11+).
fn invocation(p: CliProvider, model: Option<&str>) -> (&'static str, Vec<String>) {
    // 返回 Vec<String> (不再 &'static) 因为 model 是运行时值.
    let mut args: Vec<String> = match p {
        CliProvider::Claude => vec!["--print".into(), "--output-format".into(), "json".into()],
        CliProvider::Codex => vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-".into(),
        ],
        CliProvider::Opencode => vec!["run".into(), "-".into()],
        CliProvider::Gemini => vec!["--prompt".into(), "-".into()],
    };
    if let Some(m) = model {
        // 各家都用 `--model <name>`. claude/codex 是 long flag, opencode 也是, gemini 是 -m/--model
        // 插在已有 args 之前 (例 claude --model haiku --print)
        args.insert(0, m.into());
        args.insert(0, "--model".into());
    }
    let bin = match p {
        CliProvider::Claude => "claude",
        CliProvider::Codex => "codex",
        CliProvider::Opencode => "opencode",
        CliProvider::Gemini => "gemini",
    };
    (bin, args)
}

// v0.10.4 ADR-009: 旧 strip_empty_api_keys 移到 frank_orchestrator::worker::local
// 的 resolve_and_inject_or_strip, 它内部自动 fallback 到原逻辑。删本地 dead copy。

/// 按 provider 路由到对应 parser 抽 reply 文本 + 构造 CallReport.
///
/// - `Claude` → `ai_report::parse_claude_json` (claude --output-format json)
/// - `Codex` → `ai_report::parse_codex_jsonl` (codex --json, model_hint 取自 --model)
/// - `Gemini` / `Opencode` → 直返 raw, 无 report (TODO v0.11+ 加 parser)
///
/// 任何 parser 失败 → fallback raw stdout 当 reply, `report = None`, 永不阻塞用户拿回答.
fn extract_reply_and_report(
    provider: CliProvider,
    raw_stdout: &str,
    latency_ms: u64,
    model_hint: &str,
) -> (String, Option<frank_cred::CallReport>) {
    match provider {
        CliProvider::Claude => super::ai_report::parse_claude_json(raw_stdout, latency_ms),
        CliProvider::Codex => {
            super::ai_report::parse_codex_jsonl(raw_stdout, latency_ms, model_hint)
        }
        // TODO v0.11+: gemini/opencode token parsing — 当前 fallback 仅返 raw, 无 report.
        CliProvider::Gemini | CliProvider::Opencode => (raw_stdout.to_string(), None),
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
    fn invocation_claude_uses_json_output() {
        // v0.10.5: claude 加 `--output-format json` 给 ai_report parser 用
        let (bin, args) = invocation(CliProvider::Claude, None);
        assert_eq!(bin, "claude");
        assert_eq!(args, vec!["--print", "--output-format", "json"]);
    }

    #[test]
    fn invocation_codex_uses_json_jsonl() {
        // v0.10.5: codex 加 `--json` 输出 JSONL 流
        let (bin, args) = invocation(CliProvider::Codex, None);
        assert_eq!(bin, "codex");
        assert_eq!(args, vec!["exec", "--json", "--skip-git-repo-check", "-"]);
    }

    #[test]
    fn invocation_injects_model_when_given() {
        let (bin, args) = invocation(CliProvider::Claude, Some("haiku"));
        assert_eq!(bin, "claude");
        // --model 注入在已有 args 之前
        assert_eq!(
            args,
            vec!["--model", "haiku", "--print", "--output-format", "json"]
        );
    }
}
