//! 本地状态管理: state.json + snapshots。
//!
//! P0 day1 状态: 模块占位, 数据结构与持久化实现待 day3-4。
//!
//! # 设计要点
//!
//! - state.json 位于 `~/.frank/state.json`, 单进程访问 (file lock)
//! - 每次 install/uninstall/update 前自动建 snapshot 到 `~/.frank/snapshots/<ts>/`
//! - snapshot 保留最近 N 份, 旧的自动清理 (默认 N=10)
//! - 详见 docs/DESIGN.md §7.4.4
