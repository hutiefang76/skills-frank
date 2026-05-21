//! 本地状态管理: `state.json` 持久化已安装 skills 的记录。
//!
//! # 文件布局
//!
//! - `state.json` — `~/.frank/state.json`, 已安装/启用清单 (本模块)
//! - `snapshots/` — `~/.frank/snapshots/<ts>/`, 操作前快照 (P1 实现)
//!
//! # 子模块
//!
//! - [`store`] — [`StateData`] / [`SkillState`] / [`State`] 数据结构 + 原子读写
//!
//! `snapshot` 子模块预留给 P1 rollback。

pub mod store;

pub use store::{default_path, SkillState, State, StateData};
