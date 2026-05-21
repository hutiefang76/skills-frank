//! 统一日志与 UI 打印模块。
//!
//! # 两层分离
//!
//! - **`tracing` 日志**: 结构化, 给开发/调试看, 走 stderr, 受 `RUST_LOG` 控制
//! - **`ui::*` 函数**: 给最终用户看, 走 stdout (info/success) 或 stderr (warn/error),
//!   彩色输出, 自动检测终端能力 (重定向到文件时降级为纯文本)
//!
//! # 设计动机
//!
//! 用户明确要求"打印清晰" (ADR-001 质量基线 §3)。混用 `println!` + `eprintln!` 容易
//! 颜色/前缀不一致。本模块把所有用户面输出收敛到一处, 改样式只需改这里。

use std::io::IsTerminal;
use std::sync::Once;

use tracing_subscriber::EnvFilter;

/// 用 `Once` 保证 tracing 全局只初始化一次 (避免测试时多次 init panic)。
static INIT: Once = Once::new();

/// 初始化 tracing 日志订阅器。
///
/// 默认级别 `warn` (CLI 工具应保持安静)。用户可通过环境变量调整:
/// ```bash
/// RUST_LOG=debug frank install foo
/// RUST_LOG=frank::installer=trace frank install foo
/// ```
pub fn init() {
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("warn"));

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)       // 不打印 module path, 给用户看够清爽
            .with_level(true)
            .with_ansi(std::io::stderr().is_terminal())
            .with_writer(std::io::stderr)
            .compact()
            .init();
    });
}

/// UI 输出子模块: 给最终用户看的彩色/前缀化打印。
///
/// 全部走 `println!` / `eprintln!`, 自动处理 NO_COLOR 与非 TTY 场景
/// (`owo_colors::Stream` 帮我们判断)。
pub mod ui {
    use owo_colors::{OwoColorize, Stream};

    /// 成功消息: 绿色 `✓` 前缀, 走 stdout。
    ///
    /// 用于操作完成的肯定反馈, 例如 "skill installed"。
    pub fn success(msg: &str) {
        println!(
            "{} {}",
            "✓".if_supports_color(Stream::Stdout, |t| t.green()),
            msg
        );
    }

    /// 普通信息: 蓝色 `→` 前缀, 走 stdout。
    ///
    /// 用于进度提示, 例如 "fetching from github..."。
    pub fn info(msg: &str) {
        println!(
            "{} {}",
            "→".if_supports_color(Stream::Stdout, |t| t.bright_blue()),
            msg
        );
    }

    /// 警告: 黄色 `!` 前缀, 走 stderr (不污染 stdout 用于管道处理的输出)。
    pub fn warn(msg: &str) {
        eprintln!(
            "{} {}",
            "!".if_supports_color(Stream::Stderr, |t| t.yellow()),
            msg
        );
    }

    /// 错误: 红色 `✗` 前缀 + 红色 `ERROR` 标签, 走 stderr。
    ///
    /// 用于不可恢复的失败, main 函数捕获 anyhow::Error 时调用。
    ///
    /// 注: 不叠加 `.bold()` — owo_colors 的链式 styling 在闭包里跨返回值
    /// 会触发 borrow checker (临时值生命周期不够)。用单一颜色已经足够醒目。
    pub fn error(msg: &str) {
        eprintln!(
            "{} {} {}",
            "✗".if_supports_color(Stream::Stderr, |t| t.bright_red()),
            "ERROR".if_supports_color(Stream::Stderr, |t| t.bright_red()),
            msg
        );
    }

    /// 章节标题: 加粗, 走 stdout, 用于 list/doctor 等命令的分段。
    pub fn section(title: &str) {
        // `bold()` 单独使用 (不叠 color) 没有 borrow 问题
        println!(
            "\n{}",
            title.if_supports_color(Stream::Stdout, |t| t.bold())
        );
    }
}
