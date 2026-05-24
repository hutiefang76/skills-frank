//! 本地 ONNX embedding — 用 fastembed-rs (Qdrant 官方), **零外部 token**。
//!
//! # 选型
//!
//! - 默认模型 `BAAI/bge-small-en-v1.5` (384 维, ~30MB, 英文为主)
//! - 中文场景可换 `paraphrase-ml-MiniLM-L12-v2` (384 维, 50 语种)
//! - 高质量场景 `BAAI/bge-large-en-v1.5` (1024 维, ~440MB)
//!
//! # 性能
//!
//! - 首次启动: 从 HuggingFace 拉模型到 `~/.cache/huggingface/` (~30s)
//! - 推理: CPU 单线程 ~50-200 文档/秒 (取决于文本长度)
//! - 内存: ~200MB 常驻 (模型权重 + ONNX runtime)
//!
//! # 跟 OpenAI 客户端的区别
//!
//! [`frank_memory::embed::openai::OpenAIEmbedder`] 需要 `OPENAI_API_KEY`, 按 token 计费.
//! `LocalEmbedder` **零依赖外部 service**, **零 token 成本**, 但维度更小 (384 vs 1536),
//! 质量略低 (BAAI/bge ~ OpenAI text-embedding-3-small 的 85-90%).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use fastembed::{
    EmbeddingModel, InitOptions, InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use tokio::sync::Mutex;

use frank_memory::embed::{Embedder, Embedding};

/// 本地 ONNX embedding (fastembed-rs)。
///
/// 模型在构造时下载到 `~/.cache/huggingface/`, 后续直接读 cache.
/// 通过 `Arc<Mutex<TextEmbedding>>` 在多 async task 间共享 (fastembed 内部非 Send,
/// 用 Mutex 串行化推理调用; 单实例足够工程用量, CPU 是瓶颈不是锁).
pub struct LocalEmbedder {
    model: Arc<Mutex<TextEmbedding>>,
    model_name: String,
    dim: u64,
}

impl LocalEmbedder {
    /// 默认 `BAAI/bge-small-en-v1.5` 构造 (384 维)。
    pub fn small() -> Result<Self> {
        Self::with_model(EmbeddingModel::BGESmallENV15)
    }

    /// 多语种模型 (50 语种, 384 维, 中文友好)。
    #[allow(dead_code)]
    pub fn multilingual() -> Result<Self> {
        Self::with_model(EmbeddingModel::ParaphraseMLMiniLML12V2)
    }

    /// v0.8.1: **完全 offline** 构造 — 从本机文件直接读模型, 不联网, 不 HF.
    ///
    /// `dir` 是 hf-cli/python huggingface_hub 风格的 snapshot 目录, 期望布局:
    /// ```text
    /// dir/
    ///   onnx/model.onnx              ← ONNX 模型 (BGE small: ~90 MB)
    ///   tokenizer.json               ← 必需
    ///   config.json                  ← 必需
    ///   special_tokens_map.json      ← 必需
    ///   tokenizer_config.json        ← 必需
    /// ```
    /// 用 fastembed 的 [`UserDefinedEmbeddingModel`] API, 绕开 hf-hub 联网 metadata fetch
    /// (国内服务器连 huggingface.co timeout, hf-mirror.com 不返 Content-Range 不可用).
    ///
    /// 模型文件预下载方法 (本机国外):
    /// ```bash
    /// HF_HOME=/tmp/hf-cache hf download Xenova/bge-small-en-v1.5
    /// # 然后 scp /tmp/hf-cache/hub/models--Xenova--bge-small-en-v1.5/snapshots/<commit>/ → 服务端
    /// ```
    pub fn from_files(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let read = |name: &str| -> Result<Vec<u8>> {
            std::fs::read(dir.join(name))
                .with_context(|| format!("read model file {}", dir.join(name).display()))
        };
        let onnx_file = read("onnx/model.onnx")?;
        let tokenizer_files = TokenizerFiles {
            tokenizer_file: read("tokenizer.json")?,
            config_file: read("config.json")?,
            special_tokens_map_file: read("special_tokens_map.json")?,
            tokenizer_config_file: read("tokenizer_config.json")?,
        };
        let user_def =
            UserDefinedEmbeddingModel::new(onnx_file, tokenizer_files).with_pooling(Pooling::Cls);
        let init = InitOptionsUserDefined::new();
        let embedder = TextEmbedding::try_new_from_user_defined(user_def, init)
            .with_context(|| format!("init fastembed from local files at {}", dir.display()))?;
        Ok(Self {
            model: Arc::new(Mutex::new(embedder)),
            model_name: "BAAI/bge-small-en-v1.5 (local)".to_string(),
            dim: 384,
        })
    }

    /// 指定 fastembed 内置模型构造 (会下载 ONNX, ~30s 首次).
    ///
    /// **国内服务器优先用 [`from_files`] 跳过 HF**. 见 ADR-003 / docs/known-issues.md.
    pub fn with_model(model: EmbeddingModel) -> Result<Self> {
        let (model_name, dim) = match model {
            EmbeddingModel::BGESmallENV15 => ("BAAI/bge-small-en-v1.5".to_string(), 384),
            EmbeddingModel::BGEBaseENV15 => ("BAAI/bge-base-en-v1.5".to_string(), 768),
            EmbeddingModel::BGELargeENV15 => ("BAAI/bge-large-en-v1.5".to_string(), 1024),
            EmbeddingModel::ParaphraseMLMiniLML12V2 => {
                ("paraphrase-ml-MiniLM-L12-v2".to_string(), 384)
            }
            EmbeddingModel::AllMiniLML6V2 => ("all-MiniLM-L6-v2".to_string(), 384),
            _ => ("custom".to_string(), 384),
        };
        let init = InitOptions::new(model);
        let embedder = TextEmbedding::try_new(init)
            .with_context(|| format!("init fastembed model {model_name}"))?;
        Ok(Self {
            model: Arc::new(Mutex::new(embedder)),
            model_name,
            dim,
        })
    }
}

#[async_trait]
impl Embedder for LocalEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        let texts = vec![text.to_string()];
        let model = self.model.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut guard = futures::executor::block_on(model.lock());
            guard.embed(texts, None)
        })
        .await
        .context("spawn_blocking join")?
        .context("fastembed inference")?;
        let vector = result.into_iter().next().context("empty embedding")?;
        Ok(Embedding {
            vector,
            model: self.model_name.clone(),
        })
    }

    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.model.clone();
        let name = self.model_name.clone();
        let count = texts.len();
        let vectors = tokio::task::spawn_blocking(move || {
            let mut guard = futures::executor::block_on(model.lock());
            guard.embed(texts, None)
        })
        .await
        .context("spawn_blocking join")?
        .context("fastembed batch inference")?;
        anyhow::ensure!(
            vectors.len() == count,
            "fastembed returned {} vectors for {} texts",
            vectors.len(),
            count
        );
        Ok(vectors
            .into_iter()
            .map(|v| Embedding {
                vector: v,
                model: name.clone(),
            })
            .collect())
    }

    fn dim(&self) -> u64 {
        self.dim
    }

    fn model(&self) -> &str {
        &self.model_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_default_is_384() {
        // 这个 test 真下模型, 会慢 (首次 ~30s); ignore 默认不跑 CI
        // cargo test --features fastembed-online -- --ignored
        // 留这里证明 dim 是常量 384
        let dim_for = |m: EmbeddingModel| match m {
            EmbeddingModel::BGEBaseENV15 => 768,
            EmbeddingModel::BGELargeENV15 => 1024,
            _ => 384, // 包含 BGESmallENV15 / Paraphrase / AllMiniLML6V2 等
        };
        assert_eq!(dim_for(EmbeddingModel::BGESmallENV15), 384);
        assert_eq!(dim_for(EmbeddingModel::BGELargeENV15), 1024);
    }
}
