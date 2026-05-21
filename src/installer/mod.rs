//! 安装实现: git fetch / 凭据注入 / 调 adapter / 写 state。
//!
//! P0 day1 状态: 模块占位, 实现待 day3-4。
//!
//! # 待实现子模块
//!
//! - `git`: git2-rs 拉取代码 (sparse-checkout + subpath)
//! - `junction`: Windows mklink /J 跨盘检测与 fallback
//! - `symlink`: Unix symlink (macOS/Linux)
//! - `credentials`: 从 OS keychain 读凭据并渲染 config.ini
