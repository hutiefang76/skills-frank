//! Mock 实现: 在没有 OpenAI / Anthropic key 的环境(本地测试 / 容器先起架子)下顶替真 provider。
//!
//! 启用方式: 环境变量 `FRANK_MEMORY_MOCK=1`(由 [`crate::state::AppState::from_env`] 读取)。
//!
//! 行为:
//! - [`MockEmbedder`] 不调外网, 永远返回 16 维全 0 向量 (维度故意 ≠ OpenAI 1536, 避免误把 mock
//!   collection 当真数据;model 标 `"mock-zero"`)。
//! - [`MockExtractor`] 把输入按 `\n` 拆行, 每行去掉首尾空白; 空输入返回空 Vec。
//!
//! 这样 sync-agent 容器可以先起来占位, 等真 API key 注入后改 `FRANK_MEMORY_MOCK=0` 切真 provider。

use async_trait::async_trait;
use frank_memory::embed::{Embedder, Embedding};
use frank_memory::extract::FactExtractor;

/// Mock embedder: 16 维全 0 向量, 不发任何网络请求。
#[derive(Debug, Default, Clone, Copy)]
pub struct MockEmbedder;

#[async_trait]
impl Embedder for MockEmbedder {
    // async-trait 宏会自动给方法插 'life0 'async_trait, 跟 trait 定义对齐。
    async fn embed(&self, _text: &str) -> anyhow::Result<Embedding> {
        Ok(Embedding {
            vector: vec![0.0; 16],
            model: "mock-zero".to_string(),
        })
    }

    async fn embed_batch(&self, texts: Vec<String>) -> anyhow::Result<Vec<Embedding>> {
        Ok(texts
            .into_iter()
            .map(|_| Embedding {
                vector: vec![0.0; 16],
                model: "mock-zero".to_string(),
            })
            .collect())
    }

    fn dim(&self) -> u64 {
        16
    }

    // trait 签名是 `&self -> &str`, 必须借自 self;
    // 实际我们返回 'static 字面量, 但要匹配 trait 形态。
    #[allow(clippy::unnecessary_literal_bound)]
    fn model(&self) -> &str {
        "mock-zero"
    }
}

/// Mock 事实抽取器: 按 `\n` 拆行返回; 空输入 → 空 Vec。
#[derive(Debug, Default, Clone, Copy)]
pub struct MockExtractor;

#[async_trait]
impl FactExtractor for MockExtractor {
    async fn extract(&self, content: &str) -> anyhow::Result<Vec<String>> {
        let facts: Vec<String> = content
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect();
        Ok(facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embedder_dim_and_model() {
        let e = MockEmbedder;
        assert_eq!(e.dim(), 16);
        assert_eq!(e.model(), "mock-zero");

        let v = e.embed("anything").await.unwrap();
        assert_eq!(v.vector.len(), 16);
        assert!(v.vector.iter().all(|x| *x == 0.0));
        assert_eq!(v.model, "mock-zero");
    }

    #[tokio::test]
    async fn mock_embedder_batch_preserves_count() {
        let e = MockEmbedder;
        let out = e
            .embed_batch(vec!["a".into(), "b".into(), "c".into()])
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
        for emb in out {
            assert_eq!(emb.vector.len(), 16);
            assert_eq!(emb.model, "mock-zero");
        }
    }

    #[tokio::test]
    async fn mock_extractor_splits_on_newlines() {
        let x = MockExtractor;
        let facts = x
            .extract("line one\nline two\n\n  line three  ")
            .await
            .unwrap();
        assert_eq!(facts, vec!["line one", "line two", "line three"]);
    }

    #[tokio::test]
    async fn mock_extractor_empty_input_yields_empty_vec() {
        let x = MockExtractor;
        let facts = x.extract("").await.unwrap();
        assert!(facts.is_empty());
    }

    #[tokio::test]
    async fn mock_extractor_single_line_returns_one_fact() {
        let x = MockExtractor;
        let facts = x.extract("user prefers vim").await.unwrap();
        assert_eq!(facts, vec!["user prefers vim"]);
    }
}
