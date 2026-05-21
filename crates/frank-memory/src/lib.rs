//! `frank-memory` — Rust 重写的 mem0-同思路分布式记忆。
//!
//! # 抽象层
//!
//! - [`memory`] — 数据类型 ([`Scope`], [`MemoryRecord`], [`MemoryMatch`], [`SearchOpts`])
//! - [`store`] — [`MemoryStore`] trait + Qdrant 实现
//! - [`embed`] — [`Embedder`] trait + OpenAI 实现
//! - [`extract`] — [`FactExtractor`] trait + Anthropic Claude 实现
//! - [`client`] — 高层 [`Memory`] 门面: `add` / `search` / `get` / `update` / `delete` / `list`
//!
//! # 设计依据
//!
//! 详见 `docs/ADR/003-frank-memory-rust.md`。
//!
//! # 质量基线
//!
//! 沿用 ADR-001: `clippy::pedantic` warn, `missing_docs` warn, `unsafe_code` forbid,
//! 每文件 < 300 行。

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod client;
pub mod embed;
pub mod extract;
pub mod memory;
pub mod store;

pub use client::{Memory, MemoryConfig};
pub use embed::Embedder;
pub use extract::FactExtractor;
pub use memory::{MemoryId, MemoryMatch, MemoryRecord, Scope, SearchOpts};
pub use store::MemoryStore;
