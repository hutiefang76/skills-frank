//! Fact 提取层: 用 LLM 从一段对话/文本里抽出"声明性事实"。
//!
//! 输出是一组短句, 每句独立 embeddable, 也独立可被 retrieve。
//!
//! 实现:
//! - [`claude::ClaudeExtractor`] — Anthropic Claude (默认 `claude-haiku-4-5`, 省钱)
//! - 预留: OpenAI / 本地 LLM 等

use async_trait::async_trait;

pub mod claude;

/// LLM 抽事实接口。
#[async_trait]
pub trait FactExtractor: Send + Sync {
    /// 从 `content` 中抽取事实声明列表。
    ///
    /// 输出每条都该是: 主语+谓语+宾语形态的短句, 独立可理解。例如:
    /// - "user prefers vim over emacs"
    /// - "user's project uses Rust 1.75"
    ///
    /// 不该是: 段落 / 多句话挤一行 / 笼统总结。
    async fn extract(&self, content: &str) -> anyhow::Result<Vec<String>>;
}
