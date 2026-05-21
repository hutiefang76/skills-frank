# Frank 进度记录

> **当前状态**：P0 完整 ✅ + P5/P6 启动 (workspace + frank-memory + frank-sync-agent + qdrant on tx)
> **日期**：2026-05-21 (夜班至 23:00)
> **下次开工**：P0 Day 5 (CI smoke + v0.1.0 tag); 或继续 P5/P6 (mem0 端到端真测 / orchestrator 实现)
> **GitHub**：[github.com/hutiefang76/skills-frank](https://github.com/hutiefang76/skills-frank) · main = `clippy 0-warning + 真跑通`

## 🌙 夜班记录 (2026-05-21 21:00 → 23:00)

用户去睡觉, 我独立完成以下:

### 架构演进
- ✅ skills-frank 由单 crate 改为 **Cargo workspace** (frank-cli 搬到 crates/frank-cli)
- ✅ 新增 3 个子 crate: `frank-memory` (P5 落地) / `frank-sync-agent` (P5 服务端) / `frank-orchestrator` (P6 待建)
- ✅ 4 个新 ADR: 002 workspace / 003 frank-memory / 004 orchestrator / 005 部署 tx:8317

### 代码量 (新增)
- frank-memory: 8 文件 (memory + store + embed + extract + client), 14 单测全绿
- frank-sync-agent: 3 文件 (main + state + routes), axum REST + WS 占位
- deploy: docker-compose + Caddyfile + README

### 服务器实际部署
- `tx` 摸底: 3.3G RAM (1G 可用) / 49G/59G 磁盘 / 已跑 9 个容器
- **8317 已被你的 cli-proxy-api 占用** (systemd 守护 2 个月, 305MB), 我**改用 8318**
- 在 tx:/opt/frank/ 起了 frank-qdrant (Qdrant v1.13.0) + frank-caddy (2.10-alpine)
- UFW 已开 8318/tcp
- 本机 curl http://localhost:8318/healthz + /qdrant/healthz 全 200 ✅

### ⚠️ 待你决策 (醒来看一下)

1. **腾讯云控制台安全组** 需要开 8318/tcp 入站, 否则外网 (你的本机) 访问不到 — 我没权限改
2. **cli-proxy-api 是否替换?** 它在 8317 跑 AI 网关 + 管理 UI, 跟 frank-orchestrator 的目标重叠。
   - 选项 A: 让我用 8318 跟它并存
   - 选项 B: 停掉 cli-proxy-api, frank-stack 接管 8317
3. **OPENAI_API_KEY + ANTHROPIC_API_KEY** 后续 sync-agent 跑 mem0 端到端需要; 你给我一个或挂到 tx 环境变量
4. **frank-orchestrator 实现什么时候?** 设计文档已就绪 (ADR-004), 实现工作量约 1-2 周

### 6 个 commit 已 push (按时间序)
```
b419d3d feat(P0 day3-4): installer + 三平台 adapter + state + 4 子命令端到端跑通
8c858d3 docs: CLAUDE.md + PROGRESS Day 3-4 完成清单
0f4e852 refactor: 转 Cargo workspace, frank-cli 搬到 crates/frank-cli
99ce303 docs(ADR): 002 workspace + 003 frank-memory + 004 orchestrator + 005 部署
884bf4d feat(frank-memory): mem0 同思路 Rust 重写骨架
a2dde7a feat(frank-sync-agent + deploy): axum 服务端骨架 + qdrant 已部署到 tx
```

### 验收数据 (workspace 全量)
- cargo test --workspace --all-features: **34/34** 全绿
- cargo clippy --workspace --all-targets --all-features -- -D warnings: **0** warning
- 每文件 < 300 行 (新增文件全合规, 最大 261 = store/qdrant.rs)

---


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

## ✅ Day 2 完成 (commit `34b0565`)

- [x] `src/manifest/parser.rs` load_file + discover + merge (后覆盖前)
- [x] `src/manifest/resolver.rs` Registry: find / all / by_profile
- [x] `src/cli/list.rs` clap Args + tabled 表格输出
- [x] `manifest/public.yaml` 7 个公开 skills (2 公共 + 5 自研)
- [x] 单元测试 6/6 全通过
- [x] `frank list` 端到端真测 ✅

## ✅ 依赖升级 + clippy 收紧 (commit `2f7943e` + 后续)

- [x] serde_yaml DEPRECATED → serde_yml 0.0.12
- [x] thiserror 1 → 2.0 / tabled 0.15 → 0.17 / dirs 5 → 6
- [x] clippy --all-features -- -D warnings 0 warning ✅
- [x] CI 标准达成

## ✅ Day 3-4 完成 (commit 待提)

### installer + adapter + state + CLI 全套
- [x] `src/state/store.rs` — StateData/SkillState + 原子写 (tmp+rename) + load/save/get/put/remove/iter (5 单测)
- [x] `src/installer/git.rs` — `git2` clone/fetch/checkout + sha256(url) 前 16 字符 cache key + `ProxyOptions::auto()` (3 单测)
- [x] `src/installer/link.rs` — 跨平台 symlink (unix) / symlink_dir (win) + 幂等 remove (4 单测)
- [x] `src/installer/install.rs` — 编排器: device_allowlist → fetch → 拼 subpath → adapter 分发 → 失败回滚 + `uninstall_skill` 聚合错误
- [x] `src/adapter/{claude,codex,opencode}.rs` — 3 个 unit struct 共享 link_install/link_uninstall/link_verify helper (2 单测)
- [x] `src/cli/install.rs` — 重写: manifest → install_skill → state.put → ui::success
- [x] `src/cli/uninstall.rs` / `enable.rs` / `disable.rs` — 新增, 与 state 协同 (enable 幂等)
- [x] `src/cli/list.rs` — `--installed` 真接通, 新增 status 列 (enabled/disabled/-)
- [x] Cargo.toml — `git2` 启用 vendored-libgit2 + vendored-openssl + https + ssh; 新增 `sha2 0.10` / `gethostname 0.5`

### 验收数据 (macOS Darwin 25.4 真跑)
- ✅ `cargo test --all-features` 20/20 通过
- ✅ `cargo clippy --all-targets --all-features -- -D warnings` 0 warning
- ✅ `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --all-features` 0 warning
- ✅ CI secret-scan 本地复跑 0 命中
- ✅ `frank install probe-hello` (octocat/Hello-World 真仓) 3 平台 symlink + state.json + cache 全到位 (~5s)
- ✅ `frank disable / enable` 链路对称, link 增删正确, state.enabled 切换正确
- ✅ `frank uninstall` link 清干净, state 移除, cache 保留供复用
- ✅ `frank list --installed` 真过滤, status 列正确显示

### 不在范围 / 推后
- snapshot/rollback → P1
- credentials/keychain → 等 kdwl 私有 skill (own-public 不需要)
- health_check 跑命令 → schema 已就位, P1
- slash_command 渲染 → P1
- Windows junction (mklink /J) fallback → symlink_dir 覆盖 win10+ 开发者模式, 真踩到再加
- frank update/rollback/doctor → P1

## 📋 Day 5 待办（P0 收尾）
- [ ] CI 三平台 smoke matrix 真跑通 (Linux/Win/macOS — ubuntu 已 clippy 过, 缺 Windows symlink 实测)
- [ ] manifest 里 7 个公开 skill 仓 push 到 GitHub (现在 url 还是 404, 跑不动真测)
- [ ] 第一次 `v0.1.0` tag → release.yml 跨平台构建

---

## 🗺️ 后续 Phase

| Phase | 内容 | 预计 |
|---|---|---|
| **P1** (3 天) | `frank update` + `rollback` + `doctor` | day6-8 |
| **P2** (3 天) | docker-compose (caddy + qdrant) 上 tx — 🟢 **qdrant + caddy 已部署 2026-05-21** | 部分完成 |
| **P3** (1 周) | Tauri WebUI 或 orchestrator web SPA | week 3 |
| **P4** (1 周) | AI 自维护 PR 流程 | week 4 |
| **P5** (2 周) | **frank-memory** — mem0 同思路 Rust 重写 🟢 **骨架 + qdrant 部署完成** | 进行中 |
| **P6** (1-2 周) | **frank-orchestrator** — 多 AI Agent 协作总线 (Web UI + API) | 设计完成 |

---

## ❓ 待用户拍板（DESIGN §13 开放问题）

- [ ] **Q1**: 公司 GitLab 支持？（kdwl 同时挂 GitHub + GitLab）
- [ ] **Q4**: MCP server 启动管理是否纳入 frank？
- [ ] **Q6**: session log 上传前是否本地脱敏？

---

## 📂 当前文件树 (workspace 转换后)

```
skills-frank/
├── Cargo.toml                          ← workspace 根: members + 共享 lints/profile/deps
├── Cargo.lock
├── CLAUDE.md / PROGRESS.md / README.md
├── crates/
│   ├── frank-cli/                       ← P0 主 CLI (已完成, 14 文件, 端到端跑通)
│   │   ├── Cargo.toml / src/ / manifest/
│   ├── frank-memory/                    ← P5 mem0 重写 (8 文件, 14 单测)
│   │   ├── src/{lib,memory,client}.rs
│   │   ├── src/store/{mod,qdrant}.rs
│   │   ├── src/embed/{mod,openai}.rs
│   │   └── src/extract/{mod,claude}.rs
│   └── frank-sync-agent/                ← P5 服务端 host (3 文件, 3.8MB 二进制)
│       └── src/{main,state,routes}.rs
├── deploy/                              ← Docker Compose 部署 (已上 tx)
│   ├── docker-compose.yml
│   ├── Caddyfile
│   └── README.md
├── docs/
│   ├── DESIGN.md                        ← 1030 行 / 14 章
│   ├── ADR/
│   │   ├── 001-language-rust.md
│   │   ├── 002-cargo-workspace.md      🆕
│   │   ├── 003-frank-memory-rust.md    🆕
│   │   ├── 004-frank-orchestrator.md   🆕
│   │   └── 005-deploy-tencent-8317.md  🆕
│   ├── MEMORY-DESIGN.md                 ← v2 (被 ADR-003 取代)
│   └── RUSTROVER.md
├── .idea/                               ← 9 RunConfig + codeStyles + vcs (入仓共享)
└── .github/workflows/
    ├── ci.yml                           ← fmt + clippy + test 矩阵 + 内网扫描
    └── release.yml                      ← 6 target 跨平台 + GitHub Release
```

**累计代码 + 文档**：约 2200 行
