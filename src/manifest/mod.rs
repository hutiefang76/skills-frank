//! Skill / MCP manifest 元数据模型与解析器。
//!
//! manifest 是 frank 的"配置中心" — 所有 skill 来源/权限/依赖都在 YAML 里描述,
//! 安装逻辑从这里取数据, 不允许硬编码任何 skill 信息。
//!
//! # 文件结构
//!
//! - [`schema`]: 数据结构定义 (serde struct, 直接映射 YAML)
//! - `parser` (P0 day1-2 待实现): 加载 + 合并多个 manifest 文件
//! - `resolver` (P0 day1-2 待实现): 名字 → 完整 Skill 项的查找
//!
//! # 设计约束
//!
//! manifest schema 演进需向后兼容 (R10 风险): 通过 `schema_version` 字段 + serde
//! 默认值 / `#[serde(default)]` 兜底。详见 docs/DESIGN.md §7.1。

pub mod schema;
