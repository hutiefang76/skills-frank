# Frank 进度记录

> **当前状态**：P0 Sprint Day 1 完成 ✅
> **日期**：2026-05-21
> **下次开工**：P0 Day 2 (manifest parser + resolver)

---

## ✅ Day 1 完成清单

### 工程基建
- [x] `Cargo.toml` + 严格 `[lints]` (clippy::pedantic + missing_docs + forbid unsafe)
- [x] `rustfmt.toml` (max 100 列 + StdExternalCrate import 分组)
- [x] `.gitignore` (排除 target/, 共享 `.idea/runConfigurations/`)
- [x] `.gitattributes` (CRLF/LF 规范)
- [x] `README.md`

### Rust 项目骨架（10 个源文件，`cargo check` 通过 5.80s）
- [x] `src/main.rs` — entry + tracing init
- [x] `src/lib.rs` — 库导出 + 模块导览
- [x] `src/log.rs` — 统一 UI 着色打印 (success/info/warn/error/section)
- [x] `src/cli/mod.rs` — clap derive，8 个子命令枚举
- [x] `src/cli/install.rs` — install 命令 Args + 骨架
- [x] `src/manifest/mod.rs` + `schema.rs` — 完整 serde 数据模型 + 2 个单元测试
- [x] `src/adapter/mod.rs` — Adapter trait
- [x] `src/installer/mod.rs` — 占位
- [x] `src/state/mod.rs` — 占位

### IDE 配置（`.idea/` 入仓共享）
- [x] 9 个 `runConfigurations`: Help / List / Install Demo / Doctor / Test / Clippy / Build Release / Check / Fmt Check
- [x] `codeStyles/Project.xml` (max 100 列)
- [x] `vcs.xml` (Git 映射)

### CI/CD（`.github/workflows/`）
- [x] `ci.yml`: fmt + clippy(deny warnings) + check + test(三平台矩阵) + cargo doc + cargo audit + **内网 IP/密钥扫描**
- [x] `release.yml`: 6 个 target 跨平台 build + GitHub Release + crates.io publish (optional)

### 设计文档
- [x] `docs/DESIGN.md` — **1030 行 · 14 章** 完整设计文档
- [x] `docs/ADR/001-language-rust.md` — Rust 选型决策（含 Go/Java/Python 对比）
- [x] `docs/MEMORY-DESIGN.md` — **v2 自建版**（PG + mem0 + Tailscale，月费 0 元）
- [x] `docs/RUSTROVER.md` — IDE 使用指南

---

## 🎯 关键决策摘要（已对齐）

| ADR | 决策 | 拍板时间 |
|---|---|---|
| **ADR-001** | CLI 语言: **Rust 1.75+** | 2026-05-21 |
| **架构形态** | git + Tailscale 内网 sync agent | 2026-05-21 |
| **数据库** | **自建 PostgreSQL + pgvector** (Docker), 不付 TencentDB | 2026-05-21 |
| **分布式记忆** | 直接用 **mem0 Python 服务**, sync-agent (Rust) 做 MCP 协议适配 | 2026-05-21 |
| **跨设备访问** | **Tailscale** (免费 P2P + 自动加密) | 2026-05-21 |
| **AI 自维护** | 可写 feature/ 分支, 不可写 main | 2026-05-21 |
| **三类 skills** | public / own-public / private 三档 visibility | 2026-05-21 |
| **MVP** | P0 install/卸/列/启/禁 + 三平台同步 | 2026-05-21 |

---

## 📋 Day 2-5 待办（P0 Sprint）

### Day 2 — manifest 解析
- [ ] `src/manifest/parser.rs` 加载 + 合并多 YAML
- [ ] `src/manifest/resolver.rs` name → Skill 查找
- [ ] `manifest/public.yaml` 枚举公开 skills（doris-ops / feishu-read / nacos-config 等）
- [ ] manifest 集成测试

### Day 3-4 — installer + adapter
- [ ] `src/installer/git.rs` 用 `git2` 实现 sparse-checkout + subpath
- [ ] `src/installer/junction.rs` Windows mklink /J
- [ ] `src/installer/symlink.rs` Unix symlink
- [ ] `src/installer/credentials.rs` keychain 读凭据
- [ ] `src/adapter/{claude,codex,opencode}.rs` 三平台实现
- [ ] `src/state/store.rs` state.json 读写 + 文件锁
- [ ] `src/state/snapshot.rs` 备份/回滚

### Day 5 — 验收
- [ ] `frank install doris-ops` 三平台真测
- [ ] CI 三平台 smoke matrix 跑通
- [ ] 第一次 `v0.1.0` tag → release.yml 跨平台构建

---

## 🗺️ 后续 Phase

| Phase | 内容 | 预计 |
|---|---|---|
| **P1** (3 天) | `frank update` + `rollback` + `doctor` | day6-8 |
| **P2** (1 周) | docker-compose (PG + mem0 + sync-agent) + Tailscale 接入 | week 2 |
| **P3** (1 周) | Tauri WebUI | week 3 |
| **P4** (1 周) | AI 自维护 PR 流程 | week 4 |

---

## ❓ 待用户拍板（DESIGN §13 开放问题）

- [ ] **Q1**: 公司 GitLab 支持？（kdwl 同时挂 GitHub + GitLab）
- [ ] **Q4**: MCP server 启动管理是否纳入 frank？
- [ ] **Q6**: session log 上传前是否本地脱敏？

---

## 📂 当前文件树

```
skills-frank/
├── Cargo.toml + .gitignore + .gitattributes + rustfmt.toml
├── README.md
├── PROGRESS.md                       ← 本文件
├── src/                              ← 10 个 Rust 文件, cargo check ✅ 5.80s
│   ├── main.rs / lib.rs / log.rs
│   ├── cli/{mod,install}.rs
│   ├── manifest/{mod,schema}.rs
│   ├── adapter/mod.rs
│   ├── installer/mod.rs
│   └── state/mod.rs
├── docs/
│   ├── DESIGN.md                     ← 1030 行 / 14 章
│   ├── ADR/001-language-rust.md
│   ├── MEMORY-DESIGN.md              ← v2 自建版
│   └── RUSTROVER.md
├── .idea/                            ← 9 RunConfig + codeStyles + vcs (入仓共享)
└── .github/workflows/
    ├── ci.yml                        ← fmt + clippy + test 矩阵 + 内网扫描
    └── release.yml                   ← 6 target 跨平台 + GitHub Release
```

**累计代码 + 文档**：约 2200 行
