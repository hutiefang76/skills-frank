# RustRover / IntelliJ IDEA 开发环境配置

本项目对 RustRover（推荐）/ IntelliJ IDEA + Rust 插件 做了开箱即用配置。
克隆后用 RustRover 打开根目录即可。

## 一次性准备

1. **装 Rust toolchain**（如未装）

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   # 或 Windows: 下载 https://rustup.rs/ 的 .exe
   rustup default stable
   rustup component add clippy rustfmt rust-src rust-analyzer
   ```

2. **打开项目**

   - **RustRover**: File → Open → 选 `D:\workspace\skills-frank`
   - **IDEA**（社区/旗舰版）：先装 *Rust* 插件 → 同上

3. **首次同步**：等 IDE 自动跑 `cargo metadata`（看右下角进度条），全部就绪后可用。

## 入仓的预设配置

| 路径 | 作用 |
|---|---|
| `.idea/runConfigurations/*.xml` | 9 个 Run/Debug 配置，开箱即用 |
| `.idea/codeStyles/Project.xml` | 强制项目代码风格（与 `rustfmt.toml` 对齐） |
| `.idea/vcs.xml` | VCS 映射（Git） |
| `rustfmt.toml` | 格式化规则（max 100 列、`StdExternalCrate` 导入分组） |
| `clippy.toml`（可选） | clippy 配置 |
| `Cargo.toml` 的 `[lints]` | 项目级 lint（`clippy::pedantic` + `missing_docs`） |

## 已配置的 Run Configurations（IDE 右上角下拉可选）

| 名称 | 命令 | 用途 |
|---|---|---|
| `frank --help` | `cargo run -- --help` | 看顶层命令清单 |
| `frank list` | `cargo run -- list` | 跑 list 子命令（P0 占位） |
| `frank install doris-ops (demo)` | `cargo run -- install doris-ops --profile personal` | install 命令示范，带 `RUST_LOG=frank=debug` |
| `frank doctor` | `cargo run -- doctor` | 健康检查（P1 实现） |
| `cargo check (fast)` | `cargo check --all-targets` | 快速类型检查，写代码时常按 |
| `cargo test (all)` | `cargo test --workspace -- --nocapture` | 跑全部测试，`RUST_BACKTRACE=1` |
| `cargo clippy (strict)` | `cargo clippy --all-targets --all-features -- -D warnings` | 提交前必跑，CI 也跑这条 |
| `cargo fmt --check` | `cargo fmt --all -- --check` | 验证格式，CI 同步 |
| `cargo build --release` | `cargo build --release` | 出 release 二进制（`target/release/frank`） |

## 推荐快捷键映射

| 操作 | 推荐键 | 说明 |
|---|---|---|
| 跑当前 Run Config | `Shift+F10` | 默认 |
| Debug | `Shift+F9` | 设断点后跑 |
| 切换 Run Config | `Alt+Shift+F10` | 用得最多 |
| 重新格式化 | `Ctrl+Alt+L` | 等价 `cargo fmt` 单文件 |
| 优化 import | `Ctrl+Alt+O` | 配合 `imports_granularity = Crate` |
| 跳转定义 | `Ctrl+B` | RustRover 解析准确 |
| Find Usages | `Alt+F7` | |

## 调试技巧

1. **断点调试**：在 RustRover 右上选 Debug 模式跑任意 Run Config，命中断点。
   首次会要求 *Bundled GDB / LLDB*，按提示装即可。

2. **结构化日志查看**：所有 Run Config 都启用了 `emulateTerminal=true`，
   `tracing` 的 ANSI 颜色能正确显示。提高日志：
   ```bash
   # 在 Run Config 的 Environment Variables 加:
   RUST_LOG=frank=trace
   # 或单模块:
   RUST_LOG=frank::installer=debug,frank::cli=info
   ```

3. **clippy 实时反馈**：Settings → Languages & Frameworks → Rust → External Linters
   切到 `clippy`，保存代码就跑。

## Inspections（项目强制）

`Cargo.toml` 的 `[lints]` section 已强制：

- `clippy::pedantic` warn 级别（CI 升 deny）
- `missing_docs` warn → 每个 `pub` item 必须有 `///`
- `unsafe_code` forbid → 项目不允许 unsafe

RustRover 会高亮违反项，提交前用 `cargo clippy (strict)` Run Config 二次校验。

## 常见问题

**Q：`cargo check` 第一次很慢？**
A：要下载 100+ 个 crate，首次 3-10 分钟正常。后续秒级。

**Q：IDE 一直转圈解析？**
A：删 `target/`，重启 IDE。极端情况 File → Invalidate Caches → Invalidate and Restart。

**Q：找不到 cargo 命令？**
A：Settings → Languages & Frameworks → Rust → Toolchain location 指到 `~/.cargo/bin`。

**Q：能否用 VSCode 代替？**
A：可以。装 `rust-analyzer` + `CodeLLDB` 扩展，`.vscode/` 里有预设（待补）。

## 团队约定

- 提交代码前必须跑 `cargo clippy (strict)` 0 warning
- 提交代码前必须跑 `cargo fmt --check` 通过
- 单文件 < 300 行（拆模块）
- 每个 `pub` 必须有 `///` 文档
- 详见 [`docs/ADR/001-language-rust.md`](ADR/001-language-rust.md) 质量基线
