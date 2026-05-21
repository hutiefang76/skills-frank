# ADR-001: 选 Rust 实现 frank-cli

| Field | Value |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-05-21 |
| **Decider** | hutiefang |
| **Supersedes** | (无) |

## 背景

frank 是 CLI 工具，目标用户包括陌生人（非自用）。语言选型影响：

- 分发体验（二进制大小、安装路径、平台覆盖）
- 维护成本（学习曲线、AI 协作准确率、长期回头看代码的心智成本）
- 性能（CLI 是 IO 密集，CPU 性能权重低）
- 生态（git 库、yaml 库、CLI 框架）

## 候选

| 语言 | 二进制 | 冷启动 | 用户熟练度 | AI 协作 | 分发体验 |
|---|---|---|---|---|---|
| **Rust** | 1-3 MB | 1-5 ms | 低 | 中（rustc 报错友好） | ⭐⭐⭐⭐⭐ |
| Go | 5-15 MB | 5-20 ms | 中 | 高 | ⭐⭐⭐⭐⭐ |
| Java + GraalVM Native | 10-30 MB | 20-50 ms | 高 | 高 | ⭐⭐⭐ |
| Java + jpackage | 80-150 MB | 500-2000 ms | 高 | 高 | ⭐⭐ |
| Python + uv | 10-30 MB | 100-300 ms | 高 | 高 | ⭐⭐⭐ |

## 决策

**采用 Rust 1.75+**。

## 理由

1. **二进制最小** — 1-3 MB，其他语言至少 5 MB 起。CLI 工具分发场景这是核心
2. **冷启动最快** — 1-5 ms，用户启动体验最好
3. **npm 分发生态成熟** — biome / swc / esbuild / dprint 已验证 Rust + npm wrapper 模式
4. **包管理器全覆盖** — cargo install / brew / scoop / winget / npm，渠道最多
5. **未来扩展同栈** — sync-agent (axum) / WebUI (Tauri) 可统一 Rust 技术栈，减少认知切换
6. **AI 主笔可行** — 用户明确表态 "AI 写压力不大"，由 AI 承担学习曲线

## 代价

| 代价 | 缓解措施 |
|---|---|
| 学习曲线陡（borrow checker / lifetime） | AI 主笔；用户做 review |
| AI 写 Rust 偶发翻车 | 强制 `clippy::pedantic` + `forbid(unsafe_code)`；CI 三平台矩阵；单元测试覆盖 |
| 3 个月后回头改代码 | 全量文档注释（`#![warn(missing_docs)]`）+ 模块化（每文件 < 300 行） |
| GraalVM/JIT 无运行时反射 | 不依赖反射的库选型；用 `serde` derive |
| libgit2 系统依赖 | `git2` crate 启用 `vendored-libgit2`，无系统依赖 |

## 质量基线（落地到 Cargo.toml + CI）

用户明确要求三条，写入项目硬规范：

- ✅ **代码结构清晰**：每文件 < 300 行；模块单一职责；`clippy::pedantic` 全开
- ✅ **注释完整**：`#![warn(missing_docs)]`；每个 `pub` item 必须 `///`
- ✅ **打印清晰**：业务日志 `tracing`；UI 输出 `owo-colors` 着色；统一 `log.rs` 收敛

## 替代方案为什么不选

- **Go** — 学习曲线低、AI 协作好，但二进制大 5x、缺 cargo 一站式工具链、npm wrapper 生态例子少于 Rust
- **Java + GraalVM Native** — 用户最熟，但跨平台分发要在每个目标平台跑 GraalVM 编译（不能像 Rust `cargo build --target` 一行交叉编译）
- **Java + jpackage** — 包体 80-150 MB（含 JRE），陌生人安装门槛高
- **Python + uv** — 原型快，但分发依赖 PyOxidizer/PyInstaller，生态成熟度不如 Rust

## 后续动作

- [x] Cargo.toml 含 lint 配置
- [x] src/log.rs 实现统一打印
- [x] DESIGN.md ADR-001 同步更新
- [ ] CI workflow (GitHub Actions) 启用 clippy + missing_docs + 三平台 build
- [ ] 每 PR 必须跑 `cargo test` + `cargo clippy -- -D warnings`

## 参考

- [Rust 在 CLI 工具中的应用](https://www.rust-lang.org/what/cli)
- [biome 分发架构](https://biomejs.dev) — npm wrapper + native binary
- [cargo cross 文档](https://github.com/cross-rs/cross)
