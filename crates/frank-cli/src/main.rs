//! frank-cli 二进制入口。
//!
//! 职责非常薄: 初始化日志系统 → 解析 CLI 参数 → 调用 lib 中的 dispatcher。
//! 所有业务逻辑放在 `frank` library 里 (`src/lib.rs`), 方便被未来的 WebUI / 集成测试复用。

use std::process::ExitCode;

use frank::cli;
use frank::log as flog;

/// 二进制入口。
///
/// 返回 `ExitCode` 而非 `()`, 这样可以把业务错误码 (如 `ErrorKind::NotFound`)
/// 准确传给 shell, 方便 CI 脚本/管道判断。
fn main() -> ExitCode {
    // 第一步: 初始化 tracing 日志系统。
    // 用户可通过 RUST_LOG=debug frank ... 启用详细日志。
    // 默认级别 warn, 保持 CLI 输出干净。
    flog::init();

    // 第二步: 解析参数并 dispatch。
    // 所有用户面错误信息走 cli::run 内部的 UI 模块 (彩色), 不要在 main 里打印。
    match cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // 用 UI 模块统一着色输出 (红色 ERROR 前缀), 不直接 eprintln。
            flog::ui::error(&format!("{err:#}"));
            ExitCode::FAILURE
        }
    }
}
