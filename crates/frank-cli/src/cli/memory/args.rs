//! `frank memory` 子命令的 clap Args 结构体定义。
//!
//! 抽到独立文件给 `mod.rs` 瘦身, 保持每文件 < 300 行 (ADR-001)。

use clap::{Args as ClapArgs, Subcommand};

/// `frank memory <sub>` 总参数。clap 会把子命令树挂在这里。
#[derive(ClapArgs, Debug)]
pub struct Args {
    /// 具体的 memory 子命令。
    #[command(subcommand)]
    pub command: MemoryCommand,

    /// 显式指定 sync-agent base URL (覆盖 env 与 config)。
    #[arg(long, global = true)]
    pub agent_url: Option<String>,
}

/// memory 子命令枚举。
#[derive(Subcommand, Debug)]
pub enum MemoryCommand {
    /// 添加一段自然语言, 服务端用 LLM 抽出多条 fact。
    Add(AddArgs),

    /// 添加单条已成型的 fact (跳过 LLM, 适合脚本写入)。
    AddRaw(AddRawArgs),

    /// 向量检索: 给定 query, 返回 top-K 相关记忆。
    Search(SearchArgs),

    /// 列出指定 scope 下的记忆 (不做向量检索)。
    List(ListArgs),

    /// 按 ID 取单条记录。
    Get(GetArgs),

    /// 按 ID 删除一条记录。
    Delete(DeleteArgs),

    /// 探活: GET /healthz, 看 sync-agent 是否在线。
    Healthz,
}

/// `frank memory add` 参数。
#[derive(ClapArgs, Debug)]
pub struct AddArgs {
    /// 自然语言内容, 例如 "I prefer vim over emacs"。
    pub content: String,

    /// scope.user_id (例如 GitHub username 或邮箱)。
    #[arg(long)]
    pub user: Option<String>,

    /// scope.agent_id (例如 claude-code / codex / gemini)。
    #[arg(long)]
    pub agent: Option<String>,

    /// scope.session_id (一次会话的标识)。
    #[arg(long)]
    pub session: Option<String>,

    /// 额外 JSON 元数据 (字符串形式, 必须解析为 object)。
    #[arg(long)]
    pub metadata: Option<String>,

    /// v0.8: 客户端抽事实模式 — 调本机 cli 把 content 拆成多条独立 fact, 然后逐条 add_raw 入库.
    /// 选项: `auto` (v0.11 默认, 自动选可用 cli) / `claude` / `codex` / `gemini` / `none` (服务端抽).
    /// 借 mem0 抽 prompt 模板, 用户本机已登录 cli 复用 → 零额外 token 费.
    /// auto 优先级: FRANK_AI_PROVIDER env > claude > codex > gemini > none (兜底).
    #[arg(long, default_value = "auto")]
    pub extract_with: String,
}

/// `frank memory add-raw` 参数。
#[derive(ClapArgs, Debug)]
pub struct AddRawArgs {
    /// 单条 fact 文本。
    pub fact: String,

    /// scope.user_id。
    #[arg(long)]
    pub user: Option<String>,

    /// scope.agent_id。
    #[arg(long)]
    pub agent: Option<String>,

    /// scope.session_id。
    #[arg(long)]
    pub session: Option<String>,

    /// 额外 JSON 元数据。
    #[arg(long)]
    pub metadata: Option<String>,
}

/// `frank memory search` 参数。
#[derive(ClapArgs, Debug)]
pub struct SearchArgs {
    /// 检索 query (自然语言)。
    pub query: String,

    /// scope.user_id。
    #[arg(long)]
    pub user: Option<String>,

    /// scope.agent_id。
    #[arg(long)]
    pub agent: Option<String>,

    /// 最多返回多少条 (默认 10)。
    #[arg(long)]
    pub limit: Option<u64>,

    /// 相似度阈值, 0..1 (默认 0.5)。
    #[arg(long)]
    pub score_threshold: Option<f32>,
}

/// `frank memory list` 参数。
#[derive(ClapArgs, Debug)]
pub struct ListArgs {
    /// scope.user_id。
    #[arg(long)]
    pub user: Option<String>,

    /// scope.agent_id。
    #[arg(long)]
    pub agent: Option<String>,

    /// scope.session_id。
    #[arg(long)]
    pub session: Option<String>,

    /// 最多返回多少条 (默认 100)。
    #[arg(long, default_value_t = 100)]
    pub limit: u64,
}

/// `frank memory get` 参数。
#[derive(ClapArgs, Debug)]
pub struct GetArgs {
    /// 记忆 ID (UUID v4 字符串)。
    pub id: String,
}

/// `frank memory delete` 参数。
#[derive(ClapArgs, Debug)]
pub struct DeleteArgs {
    /// 记忆 ID (UUID v4 字符串)。
    pub id: String,
}
