# Frank — AI 工具链治理平台 · 设计文档

| Field | Value |
|---|---|
| **Document type** | Engineering Design Doc (RFC-style) |
| **Status** | Draft v1.0 — 待 review |
| **Author** | hutiefang |
| **Reviewer** | TBD |
| **Created** | 2026-05-21 |
| **Last updated** | 2026-05-21 (P5/P6 加入) |
| **Repo** | `git@github.com:hutiefang76/skills-frank.git` |
| **Local path** | `D:\workspace\skills-frank\` |

---

## 目录

- [0. TL;DR](#0-tldr)
- [1. 背景与动机](#1-背景与动机)
- [2. 目标与非目标](#2-目标与非目标)
- [3. 用户故事](#3-用户故事)
- [4. 总体架构](#4-总体架构)
- [5. 核心概念定义](#5-核心概念定义)
- [6. 模块详细设计](#6-模块详细设计)
- [7. 数据模型](#7-数据模型)
- [8. 安全设计](#8-安全设计)
- [9. AI 自维护机制](#9-ai-自维护机制)
- [10. 演进路线](#10-演进路线)
- [11. 风险登记](#11-风险登记)
- [12. ADR（架构决策记录）](#12-adr-架构决策记录)
- [13. 开放问题](#13-开放问题)
- [14. 附录](#14-附录)

---

## 0. TL;DR

**Frank 是一个跨平台 AI 工具链治理平台**，统一管理 Claude Code / codex / opencode 三个 CLI 的 skills 与 MCP 配置。

**核心抓手**：
1. **一键 install/uninstall/list/enable/disable/update/rollback** skills 与 MCP
2. **三类来源**（公共 · 自研 · 公司）权限分仓 + 凭据隔离
3. **分布式记忆**通过腾讯云 VM Docker(qdrant + caddy + frank-sync-agent),mem0-同思路 Rust 实现 (P5)
4. **多 Agent 协作总线** (P6,ADR-004): 替代 CCB tmux 路线,浏览器 Web UI + axum API
5. **AI 可写 feature 分支**(禁写 main)实现自维护
6. **三种交互方式**：`frank` CLI / Slash Command / WebUI(P3)

**技术栈**：Rust 1.75+ Cargo workspace · 子 crate (frank-cli / frank-memory / frank-sync-agent / frank-orchestrator) · 腾讯云 VM Docker (Caddy + Qdrant + axum,跑在 tx:8318) · Tauri 或静态 SPA (WebUI,P3)

**MVP 时间**：P0 完成 + P5/P6 启动；后续 P1/P3/P4 排队

---

## 1. 背景与动机

### 1.1 现状痛点

| 痛点 | 来源 | 影响 |
|---|---|---|
| 三个 AI CLI 各自维护 skills，重复劳动 | `~/.claude/skills/` `~/.codex/skills/` `~/.opencode/skills/` | 改一处要同步三处，kdwl 项目已验证 |
| 公司 / 自研 / 公开 skills 混仓，泄露风险 | kdwl 早期把 internal/ 混在公开 repo | 已踩坑：内网 IP 泄露到公开 GitHub |
| 凭据散落各 skill 目录，明文 | `vehicle-events/code/config.ini` 等 | 多次误 commit credentials.ini |
| 跨设备同步靠手动 git pull + 手动编辑 CLAUDE.md | 你在家/公司两台机器 | 规则漂移、记忆断裂 |
| skill 安装失败无回滚，手动恢复 | `install.bat` 早期版本 | kdwl 已通过 `state.ps1` 部分解决 |
| 没有 skill 调用统计，无法优化 trigger | 全靠人工 review | undertrigger 找不到证据 |
| AI 改 skill 没流程，要么不让改要么乱改 | 当前是人工维护 | AI 自治潜力浪费 |

### 1.2 业务目标

- **G1（效率）**：单条命令完成跨三平台 skill 安装，目标 < 10s
- **G2（安全）**：公司 skills **零误泄露**到公开 repo（强制 CI 扫描 + 分仓）
- **G3（可恢复）**：任何 skill 操作 60 秒内可回滚到上一版本
- **G4（可观测）**：每个 skill 的调用次数 / trigger 成功率可查
- **G5（多设备一致）**：两台设备的 CLAUDE.md + memory + 启用列表自动同步
- **G6（AI 自助）**：AI 发现 skill bug 时可自动提 PR 到 feature 分支

---

## 2. 目标与非目标

### 2.1 In Scope（覆盖用户提的 10 项需求）

| 需求 | 设计模块 | Phase |
|---|---|---|
| 1. 一键安装常用 skills/MCP | `frank install` + manifest | P0 |
| 2. 自动化更新 | `frank update` + 定时任务 | P1 |
| 3. 一键启用/禁用 | `frank enable/disable` + state.json | P0 |
| 4. 分布式记忆 (v1: 腾讯云四件套 → v2: 自建 Docker stack 见 ADR-003) | sync-agent + Qdrant + Postgres | P5 (重新规划) |
| 5. CLI / UI / Slash 三种入口 | frank-cli / Tauri webui / slash-commands | P0 / P3 / P0 |
| 6. AI 自动拉取 + 使用中维护 | MCP server adapter + PR bot | P4 |
| 7. 低资源安全更新 + 回滚 | snapshot before update + `frank rollback` | P1 |
| 8. 支持 Claude/codex/opencode CLI + app | 三个 adapter | P0 |
| 9. 其他建议（见下） | 见下 | 持续 |
| 10. 公共 + 自研 + 公司 三类来源 | 三 visibility + 分 manifest | P0 |
| 11. 分布式记忆 Rust 化 (mem0 同思路) | `frank-memory` crate + Qdrant + LLM 抽取 | **P5** → [ADR-003](ADR/003-frank-memory-rust.md) |
| 12. 多 Agent 协作总线 (替代 CCB tmux) | `frank-orchestrator` crate + Web UI + axum WS | **P6** → [ADR-004](ADR/004-frank-orchestrator.md) |

**第 9 项"其他建议"**（设计期已并入）：
- manifest-driven 元数据
- 凭据 KMS 加密
- 公私分仓
- per-skill health-check
- 设备级版本锁
- MCP 与 skill 分离治理
- GitHub Actions 三平台 smoke test 矩阵

### 2.2 Out of Scope

- ❌ 不重写 Claude Code / codex / opencode 本体
- ❌ 不做 LLM 模型路由（用户自己用各家 CLI）
- ❌ 不做团队级 RBAC（个人 + 公司双角色够用）
- ❌ 不实现 skill 编写 IDE（用 skill-creator 即可）
- ❌ 不做付费订阅 / SaaS 多租户

---

## 3. 用户故事

**US-1**：作为一名同时用 Claude Code 和 codex 的工程师，我希望一条命令把 `doris-ops` 同时装到两个平台上，不用复制粘贴文件。

**US-2**：作为有两台设备的开发者（家里 + 公司），我希望在家启用了某个 skill，公司机器自动同步该启用状态，且 CLAUDE.md 规则保持一致。

**US-3**：作为有公司项目的工程师，我希望公司 skills（含内网 IP / 业务代码）**绝对不会**因为我手滑被 push 到公开 repo。

**US-4**：作为踩过坑的人，我希望升级一个 skill 失败时，能在 60 秒内回滚到上一版本，不影响其他 skill。

**US-5**：作为想知道 skills 效果的产品经理（这里就是我自己），我希望能看到哪些 skill 被调用了多少次、哪些 trigger 失败了，作为优化 description 的依据。

**US-6**：作为信任 AI 的开发者，我希望 AI 发现 skill 描述不准时，能自动改并提交到 `feature/ai-update-*` 分支，等我 review。

**US-7**：作为关心安全的用户，我希望腾讯云上存的 memory / log / 凭据都是加密的，云厂商运维也看不到内容。

**US-8**：作为新接触 AI 工具的人，我希望有 WebUI 可视化操作（不强迫学 CLI）。

---

## 4. 总体架构

### 4.1 三层架构概览

```
┌─────────────────────────────────────────────────────────────────────────┐
│ L3 入口层 (User Interface)                                              │
│   ┌──────────────┐  ┌────────────────────┐  ┌──────────────────────┐   │
│   │  frank CLI   │  │ /frank Slash Cmd   │  │  Tauri WebUI (P3)    │   │
│   │  (Rust bin)  │  │  (3 platforms)     │  │  React + Rust shell  │   │
│   └──────┬───────┘  └─────────┬──────────┘  └──────────┬───────────┘   │
└──────────┼─────────────────────┼──────────────────────────┼────────────┘
           ↓                     ↓                          ↓
┌─────────────────────────────────────────────────────────────────────────┐
│ L2 治理层 (Governance Core)                                             │
│   ┌────────────────────┐  ┌────────────────────┐  ┌──────────────────┐ │
│   │  manifest engine   │  │   adapter layer    │  │  state manager   │ │
│   │  - parse           │  │  - claude-adapter  │  │  - state.json    │ │
│   │  - resolve sources │  │  - codex-adapter   │  │  - snapshots/    │ │
│   │  - merge profiles  │  │  - opencode-adapter│  │  - rollback      │ │
│   └────────────────────┘  └────────────────────┘  └──────────────────┘ │
│   ┌────────────────────┐  ┌────────────────────┐  ┌──────────────────┐ │
│   │  installer         │  │  health-check      │  │  sync-client     │ │
│   │  - git clone       │  │  - per-skill probe │  │  - to TencentCloud│ │
│   │  - junction render │  │  - dependency check│  │  - encrypt/decrypt│ │
│   │  - credentials inject│  - network probe   │  │  - resolve diff  │ │
│   └────────────────────┘  └────────────────────┘  └──────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
           ↓ (git fetch)              ↓ (sync)             ↑ (download)
┌─────────────────────────────────────────────────────────────────────────┐
│ L1 存储层 (Storage)                                                     │
│   ┌─────────────────┐  ┌──────────────────┐  ┌─────────────────────┐   │
│   │ 公共 skills      │  │ 自研 skills      │  │ 公司 skills          │   │
│   │ upstream GitHub  │  │ 个人 GitHub repo │  │ private repo (kdwl)  │   │
│   │ - anthropics/*   │  │ - doris-ops      │  │ - skills-kdwl        │   │
│   │ - skill-creator  │  │ - feishu-read    │  │ - 公司 GitLab        │   │
│   │ - superpowers    │  │ - nacos-config   │  │ ⚠ 内网拓扑严禁公开    │   │
│   └─────────────────┘  └──────────────────┘  └─────────────────────┘   │
│                                                                          │
│   ┌─────────────────────────────────────────────────────────────────┐  │
│   │ 腾讯云 VM (tx, 101.35.227.232) — Docker Compose 编排 (ADR-005)   │  │
│   │   唯一对外端口: 8318 (TLS 终止 by Caddy)                          │  │
│   │                                                                 │  │
│   │     ┌──────────┐    ┌──────────┐    ┌─────────────────┐        │  │
│   │     │  Caddy   │←───│  qdrant  │    │ frank-sync-agent│        │  │
│   │     │ :8318→   │    │ :6333    │←───│  (axum REST+WS) │        │  │
│   │     │ /memory/ │    │ :6334    │    │  :3000 内部     │        │  │
│   │     │ /orchstr/│    │ vec DB   │    │  /memory + /orch│        │  │
│   │     │ /qdrant/ │    └──────────┘    └─────────────────┘        │  │
│   │     └──────────┘                          │                    │  │
│   │                              ┌─────────────┘                    │  │
│   │                              ↓                                  │  │
│   │                         ┌──────────┐                            │  │
│   │                         │ Postgres │  (P6 orchestrator 启用)    │  │
│   │                         │ :5432    │                            │  │
│   │                         └──────────┘                            │  │
│   │                                                                 │  │
│   │ 当前状态: caddy + qdrant 已部署 (2026-05-21); sync-agent 骨架就绪│  │
│   └─────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

> 注: 原 v1 设计中的 TencentDB / COS / KMS 四件套已由自建 Docker stack 取代,详见 ADR-003/005 与 §15 文档版本演进。

### 4.2 数据流（典型场景：装一个公司 skill）

> 注: 下方流程是 v1 概念草图(仍能说明 install 端到端经过哪些组件); sync-agent 一段已由 ADR-003 的"frank-memory + axum REST"替代,KMS / TencentDB 一段在 v2 已不适用。

```
用户: $ frank install kdwl:vehicle-events
  │
  ├─► frank-cli 解析参数
  │     - skill name = kdwl:vehicle-events
  │     - 查 manifest 索引
  │
  ├─► manifest engine 解析
  │     - 在 ~/.frank/manifests/company-kdwl.yaml 找到
  │     - visibility = private
  │     - source = git@github.com:hutiefang76/skills-kdwl.git
  │     - subpath = internal/vehicle-events
  │     - require_network = openvpn
  │
  ├─► health-check 前置
  │     - VPN 连通? OK
  │     - 凭据可用? 读取 Windows Credential Manager
  │     - 设备在 allowlist? OK
  │
  ├─► state manager 创建 snapshot
  │     - 备份当前三平台 ~/.{claude,codex,opencode}/skills/ 白名单内容
  │     - 写入 .frank/snapshots/2026-05-21T17-40-00/
  │
  ├─► installer 拉取
  │     - git sparse-checkout internal/vehicle-events
  │     - 渲染 credentials → config.ini（注入 keychain 凭据）
  │     - 装 python 依赖到共享 venv
  │
  ├─► adapter 三平台分发
  │     - claude-adapter: mklink /J ~/.claude/skills/kdwl:vehicle-events ...
  │     - codex-adapter:  mklink /J ~/.codex/skills/... + 渲染 prompts/
  │     - opencode-adapter: mklink /J ~/.opencode/skills/...
  │
  ├─► state.json 更新
  │     - enabled: true, version: <git sha>, installed_at: <ts>
  │
  ├─► sync-client 上报
  │     - POST /api/v1/state/sync → 腾讯云 sync-agent
  │     - 加密（KMS）后写 TencentDB
  │
  └─► 输出: ✓ kdwl:vehicle-events installed (3 platforms, 1.2s)
```

### 4.3 部署拓扑

> ⚠️ 下方为 v1 部署拓扑(Go / TencentDB / COS / KMS),已被 §4.1 末尾的实际拓扑图与 [ADR-005](ADR/005-deploy-tencent-8317.md) 取代。保留供历史对比;**实际部署看 §4.1**。

```
┌──────────────────────────┐         ┌──────────────────────────┐
│  设备 A (家里 Windows)    │         │  设备 B (公司 Mac)        │
│                          │         │                          │
│  frank-cli (Go binary)   │         │  frank-cli (Go binary)   │
│  ~/.frank/               │         │  ~/.frank/               │
│  ├─ manifests/           │         │  ├─ manifests/           │
│  ├─ state.json           │         │  ├─ state.json           │
│  ├─ snapshots/           │         │  ├─ snapshots/           │
│  └─ credentials (Cred Mgr/Keychain)  └─ ...                   │
└────────────┬─────────────┘         └────────────┬─────────────┘
             │ HTTPS + mTLS                       │
             │  (device cert)                     │
             └─────────────────┬──────────────────┘
                               ↓
        ┌──────────────────────────────────────────────┐
        │  腾讯云 (北京 / 上海可用区)                   │
        │                                              │
        │  ┌──────────────────────────────────────┐   │
        │  │  CVM 2核4G                           │   │
        │  │  - sync-agent (Go HTTP server)       │   │
        │  │  - port 443 (Nginx + cert)           │   │
        │  └──────────────┬───────────────────────┘   │
        │                 ↓                            │
        │  ┌────────────┐ ┌────────────┐ ┌──────────┐│
        │  │ TencentDB  │ │ TencentDB  │ │   COS    ││
        │  │ PostgreSQL │ │   MySQL    │ │ (bucket) ││
        │  │ memory     │ │ statistics │ │ logs/    ││
        │  │            │ │            │ │ rules/   ││
        │  └────────────┘ └────────────┘ └──────────┘│
        │                                              │
        │  ┌─────────────────────────────────────┐    │
        │  │  KMS (密钥管理)                      │    │
        │  │  - master key for credentials       │    │
        │  │  - device certs CA                  │    │
        │  └─────────────────────────────────────┘    │
        └──────────────────────────────────────────────┘
```

---

## 5. 核心概念定义

| 术语 | 定义 |
|---|---|
| **skill** | 一组 prompt + 资源文件，触发 LLM 行为。三平台格式略不同，但 SKILL.md 为通用核心 |
| **MCP** | Model Context Protocol server，长驻进程对外暴露 tools。与 skill 是两套机制 |
| **manifest** | 描述 skill/MCP 元数据的 YAML 文件，含 source/visibility/auth/profile |
| **adapter** | 把通用 skill 渲染成三平台各自格式的转换器 |
| **profile** | 一组 manifest 的集合，按身份（personal/company）或设备分组 |
| **visibility** | v0.2 两层 5 档：**frank 内置** (`frank-own` 芳哥自研, `frank-recommended` 芳哥推荐) + **用户自定义** (`user-public` 用户开源, `user-company` 用户公司, `user-private` 用户私有)。v0.1 三档 `public`/`own-public`/`private` 通过 serde alias 兼容 |
| **device-allowlist** | manifest 项可指定只在特定 hostname 设备生效 |
| **snapshot** | 操作前的状态快照，含三平台 skills 目录 + state.json |
| **sync-agent** | 跑在腾讯云的 Go HTTP 服务，承担四类记忆存储 |
| **health-check** | 每个 skill 自带的探针脚本（依赖/网络/凭据），fail-fast |

---

## 6. 模块详细设计

### 6.1 frank-cli（Go）

#### 6.1.1 目录结构

```
skills-frank/
├── cmd/frank/                    # CLI entry point
│   └── main.go
├── internal/
│   ├── manifest/                 # manifest 解析与合并
│   │   ├── schema.go
│   │   ├── parser.go
│   │   └── resolver.go
│   ├── adapter/                  # 三平台适配
│   │   ├── claude.go
│   │   ├── codex.go
│   │   └── opencode.go
│   ├── installer/                # 安装/卸载实现
│   │   ├── git.go
│   │   ├── junction.go (Windows)
│   │   ├── symlink.go  (Unix)
│   │   └── credentials.go
│   ├── state/                    # 状态管理
│   │   ├── store.go
│   │   ├── snapshot.go
│   │   └── rollback.go
│   ├── health/                   # 健康检查
│   │   └── check.go
│   ├── sync/                     # 腾讯云同步客户端
│   │   ├── client.go
│   │   ├── crypto.go
│   │   └── conflict.go
│   └── tui/                      # 终端 UI（list 命令用）
│       └── render.go
├── pkg/                          # 公开接口（给 WebUI 复用）
│   └── api/
├── manifest/                     # 公开 manifest（入 git）
│   ├── public.yaml
│   └── private.example.yaml
├── adapters/                     # 平台特定模板
│   ├── claude/
│   ├── codex/
│   └── opencode/
├── sync-agent/                   # 服务端（独立编译）
│   ├── cmd/
│   ├── internal/
│   └── deploy/
├── webui/                        # Tauri (P3)
├── scripts/                      # CI / dev tools
├── docs/
│   ├── DESIGN.md                 # 本文档
│   ├── INSTALL.md
│   ├── ADR/
│   └── USAGE.md
├── .github/workflows/            # GitHub Actions (smoke matrix)
├── .gitignore
├── .gitattributes
├── go.mod
├── go.sum
├── LICENSE                       # MIT
└── README.md
```

#### 6.1.2 命令清单

| 命令 | 作用 | P-Phase | 示例 |
|---|---|---|---|
| `frank install <name>` | 安装一个 skill/MCP | P0 | `frank install doris-ops` |
| `frank install --all [--profile p]` | 批量安装 manifest 中所有 | P0 | `frank install --all --profile company` |
| `frank uninstall <name>` | 卸载 | P0 | `frank uninstall doris-ops` |
| `frank list [--profile p] [--installed]` | 列出 skills（TUI 表格） | P0 | `frank list --installed` |
| `frank enable <name>` | 启用已装的 skill | P0 | `frank enable kdwl:vehicle-events` |
| `frank disable <name>` | 禁用（保留文件，仅从 adapter 卸载） | P0 | `frank disable old-skill` |
| `frank update [<name>]` | 更新指定/全部 skill | P1 | `frank update --all` |
| `frank rollback [<name>] [--to <ts>]` | 回滚 | P1 | `frank rollback --to 2026-05-21T17-40` |
| `frank doctor` | 诊断（依赖/网络/凭据） | P1 | `frank doctor` |
| `frank sync [push|pull]` | 强制同步腾讯云 | P2 | `frank sync pull` |
| `frank memory <query>` | 查询分布式记忆 | P2 | `frank memory "doris credentials"` |
| `frank stats [--skill <name>]` | skill 调用统计 | P2 | `frank stats --skill doris-ops` |
| `frank ui` | 启动 WebUI | P3 | `frank ui` |
| `frank ai-suggest <skill>` | AI 提 PR 改进 skill | P4 | `frank ai-suggest doris-ops` |

#### 6.1.3 CLI 框架选型

> 注: v1 拟用 Go (cobra + viper + lipgloss + go-git + yaml.v3); 实际按 [ADR-001](ADR/001-language-rust.md) 改 Rust, 等效栈见下。

- **clap (derive)**(CLI 解析,支持 env / default / subcommand)+ **owo-colors**(TUI 着色)+ **tabled**(`frank list` 表格)+ **git2** (vendored libgit2, 无系统依赖) + **serde_yml**(manifest 解析,serde_yaml fork)
- 单二进制,cross-compile 矩阵 (release.yml):`x86_64-unknown-linux-gnu` / `x86_64-pc-windows-msvc` / `x86_64-apple-darwin` / `aarch64-apple-darwin` (+ ubuntu/win/macos CI test matrix)

### 6.2 manifest 系统

#### 6.2.1 schema 概览（详见 §7.1）

每个 skill 在 manifest 中是一个 item，含：
- `name`（唯一标识，namespace 用 `:` 分隔）
- `source`（git url / local path）
- `visibility`（public / own-public / private）
- `auth`（认证方式 + 凭据指针）
- `target_platforms`（默认全部，可指定）
- `profile`（personal / company / 自定义）
- `device_allowlist`（hostname 列表）
- `require_network`（vpn / corp-net / none）
- `dependencies`（python pkgs / system bin）
- `health_check`（探针命令）
- `slash_command`（可选，注册到 Claude/codex）

#### 6.2.2 多 manifest 合并规则

frank 启动时按顺序加载：
1. `skills-frank/manifest/public.yaml`（公开仓内）
2. `~/.frank/manifests/*.yaml`（本地私有，不入仓）
3. 环境变量 `FRANK_EXTRA_MANIFEST` 指向的额外文件

冲突规则：**后加载覆盖前加载**（本地 > 公开）

#### 6.2.3 三类 visibility 行为

| visibility | 存放位置 | git 操作 | 凭据 |
|---|---|---|---|
| `public` | 公开 repo / upstream | clone via HTTPS | 无需 |
| `own-public` | 你的开源 repo | clone via HTTPS/SSH | 可选 PAT |
| `private` | 私有 repo | clone via SSH only | 必须 keychain |

### 6.3 adapter 层

每个 adapter 实现接口：

```go
type Adapter interface {
    Name() string                                   // "claude" / "codex" / "opencode"
    Install(skill *Skill, dest string) error        // 渲染到目标平台
    Uninstall(skill *Skill) error
    Enable(skill *Skill) error
    Disable(skill *Skill) error
    Verify(skill *Skill) error                      // 验证已安装
    PlatformDir() string                            // ~/.claude/skills/
}
```

#### 6.3.1 claude-adapter

- 目标：`~/.claude/skills/<name>/` + `~/.claude/commands/<name>.md`（如有 slash）
- 实现：Windows 用 `mklink /J`，Unix 用 symlink；slash command 是真实 .md 文件（小，复制即可）

#### 6.3.2 codex-adapter

- 目标：`~/.codex/skills/<name>/` + `~/.codex/prompts/<name>.md`
- 注意：codex 的 yaml 可能有不同字段，adapter 负责转换

#### 6.3.3 opencode-adapter

- 目标：`~/.opencode/skills/<name>/`
- 无 slash 概念，仅装 skill 本体

### 6.4 sync-agent（腾讯云服务端）

#### 6.4.1 服务架构

- 单二进制 Go 服务，跑在 2核4G CVM
- Nginx 反代 + Let's Encrypt cert
- mTLS：客户端必须出示设备证书（由 KMS 签发）
- 数据库连接：内网 VPC，不走公网

#### 6.4.2 API 设计（详见 §7.4）

```
POST /api/v1/state/sync      → 上报本机 state.json
GET  /api/v1/state/diff      → 拉取与云端 diff
POST /api/v1/memory/entity   → memory MCP 写
GET  /api/v1/memory/query    → memory MCP 读
POST /api/v1/log/upload      → session log 上传 (COS pre-signed)
POST /api/v1/stats/event     → skill 调用事件
GET  /api/v1/stats/summary   → 统计聚合
POST /api/v1/rules/upload    → CLAUDE.md 上传
GET  /api/v1/rules/list      → 拉取规则
POST /api/v1/device/register → 设备首次注册（签发证书）
```

#### 6.4.3 数据加密

- **传输**：mTLS（设备证书 + 服务证书）
- **静态**：所有写入 DB 的内容先经 KMS 加密；COS 启用 SSE-KMS
- **客户端解密**：只在本机内存解密，落盘前重加密

### 6.5 WebUI（Tauri，P3）

- **技术栈**：Tauri (Rust shell) + React + TanStack Query + shadcn/ui
- **核心页面**：
  - Dashboard（已装 skills 卡片 + 状态指示）
  - Marketplace（manifest 浏览 + 一键装）
  - Stats（图表：调用次数 / trigger 成功率）
  - Memory Explorer（实体/关系图可视化）
  - Settings（profile / 凭据 / 同步配置）
- **后端**：调用 `frank` CLI（subprocess）或通过 IPC 调 internal API

---

## 7. 数据模型

### 7.1 manifest schema（完整）

```yaml
# manifest/schema.yaml — JSON Schema-like 描述
schema_version: 1

# Skill 条目
skill:
  required: [name, source, visibility]
  fields:
    name:
      type: string
      pattern: '^([a-z][a-z0-9-]*:)?[a-z][a-z0-9-]*$'
      examples: ["doris-ops", "kdwl:vehicle-events"]

    description:
      type: string
      max_length: 200

    source:
      type: object
      required: [type]
      oneOf:
        - type: git
          url: string                    # git@github.com:... 或 https://...
          ref: string                    # branch / tag / commit, default: main
          subpath: string                # 多 skill 单仓时指定子目录
        - type: local
          path: string                   # 绝对路径
        - type: upstream
          parent: string                 # 引用其他 manifest 中的 skill

    visibility:
      enum: [public, own-public, private]

    auth:
      type: object
      fields:
        method: { enum: [none, ssh-key, github-pat, oauth] }
        key_ref: string                  # keychain key 名，不存明文
        require_mfa: boolean             # 公司 skills 默认 true

    target_platforms:
      type: array
      default: [claude, codex, opencode]
      items: { enum: [claude, codex, opencode] }

    profile:
      type: string
      default: personal
      examples: [personal, company, experimental]

    device_allowlist:
      type: array
      items: string                       # hostname

    require_network:
      enum: [none, internet, vpn, corp-net]

    dependencies:
      type: object
      fields:
        python: array                    # ["pymongo>=4.0", ...]
        system: array                    # ["git", "openvpn"]
        mcp: array                       # 依赖的其他 MCP server

    health_check:
      type: object
      fields:
        cmd: string                      # 退出码 0 = 健康
        timeout_seconds: integer
        run_before_install: boolean
        run_periodically: cron-expr

    slash_command:
      type: object
      fields:
        enabled: boolean
        name: string                     # default: skill name
        platforms: array                 # claude / codex

    mcp_server:
      type: object                       # 如果这是个 MCP server 而非 skill
      fields:
        command: array                   # ["node", "server.js"]
        env: map<string,string>

    metadata:
      type: object
      fields:
        author: string
        version: string                  # semver
        license: string
        homepage: string
        tags: array

# Profile 全局配置
profile:
  fields:
    name: string
    description: string
    default: boolean
    inherits: string                     # 继承另一个 profile
```

### 7.2 公开 manifest 示例（`manifest/public.yaml`）

```yaml
schema_version: 1
profile: personal

skills:
  - name: skill-creator
    description: Meta-skill for creating new skills
    source:
      type: git
      url: https://github.com/anthropics/skill-creator.git
    visibility: public
    target_platforms: [claude, codex, opencode]

  - name: superpowers
    source:
      type: git
      url: https://github.com/anthropics/superpowers.git
    visibility: public
    target_platforms: [claude]            # codex/opencode 无 plugin 概念

  - name: doris-ops
    description: TCHouse-D 运维（查表、性能、扩容）
    source:
      type: git
      url: https://github.com/hutiefang76/skills-doris-ops.git
    visibility: own-public
    dependencies:
      python: ["mysql-connector-python>=8.0"]
    health_check:
      cmd: "python -c 'import mysql.connector'"

  # ... 其他公共 / 自研 skills
```

### 7.3 私有 manifest 示例（`~/.frank/manifests/company-kdwl.yaml`）

```yaml
schema_version: 1
profile: company

skills:
  - name: kdwl:vehicle-events
    description: 车辆事件 MongoDB 查询 + 控车成功率
    source:
      type: git
      url: git@github.com:hutiefang76/skills-kdwl.git
      ref: main
      subpath: internal/vehicle-events
    visibility: private
    auth:
      method: ssh-key
      key_ref: "id_ed25519_personal"     # 指向 keychain
      require_mfa: false
    require_network: vpn
    device_allowlist:
      - ATHENA-LAPTOP                     # 你的设备 hostname
      - MAC-WORK
    dependencies:
      python: ["pymongo>=4.0"]
    health_check:
      cmd: "python -c 'import pymongo' && ping -n 1 -w 1000 10.0.1.196"
      run_before_install: true
    slash_command:
      enabled: true
      platforms: [claude, codex]

  - name: kdwl:event-protocol
    source:
      type: git
      url: git@github.com:hutiefang76/skills-kdwl.git
      subpath: internal/event-protocol
    visibility: private
    # 纯文档 skill, 无依赖
```

### 7.4 v1 腾讯云存储 schema (历史保留)

> ⚠️ 下面 §7.4.1 ~ §7.4.3 是 v1 设计的 TencentDB / COS schema, 已被 [ADR-003 frank-memory](ADR/003-frank-memory-rust.md) (Qdrant collection) 与 [ADR-004 orchestrator](ADR/004-frank-orchestrator.md) (Postgres job 表) 取代。
> §7.4.4 本地 state.json 仍然有效。
> 保留这部分供历史对比 + 后续若回到关系型存储时复用。

#### 7.4.1 TencentDB PostgreSQL（memory MCP 后端）

```sql
CREATE TABLE entities (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    name VARCHAR(256) NOT NULL,
    entity_type VARCHAR(64) NOT NULL,
    observations_encrypted BYTEA,           -- KMS 加密的 JSONB
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(user_id, name)
);

CREATE TABLE relations (
    id BIGSERIAL PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    from_entity VARCHAR(256) NOT NULL,
    to_entity   VARCHAR(256) NOT NULL,
    relation_type VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (user_id, from_entity) REFERENCES entities(user_id, name),
    FOREIGN KEY (user_id, to_entity)   REFERENCES entities(user_id, name)
);

CREATE INDEX idx_entities_user_type ON entities(user_id, entity_type);
CREATE INDEX idx_relations_from ON relations(user_id, from_entity);
```

#### 7.4.2 TencentDB MySQL（skill 调用统计）

```sql
CREATE TABLE skill_events (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id VARCHAR(64) NOT NULL,
    device_id VARCHAR(64) NOT NULL,
    skill_name VARCHAR(128) NOT NULL,
    platform ENUM('claude','codex','opencode') NOT NULL,
    event_type ENUM('trigger','complete','fail') NOT NULL,
    duration_ms INT,
    error_kind VARCHAR(64),               -- 失败时的分类
    metadata_json JSON,                   -- 不含敏感数据
    ts TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_user_skill_ts (user_id, skill_name, ts),
    INDEX idx_ts (ts)
) PARTITION BY RANGE (UNIX_TIMESTAMP(ts)) (
    -- 按月分区，旧数据自动归档
);

CREATE TABLE skill_summary_daily (         -- 聚合表，加速查询
    user_id VARCHAR(64),
    skill_name VARCHAR(128),
    date DATE,
    trigger_count INT,
    complete_count INT,
    fail_count INT,
    avg_duration_ms INT,
    PRIMARY KEY (user_id, skill_name, date)
);
```

#### 7.4.3 COS 对象存储布局

```
cos://frank-storage-<userid>/
├── logs/                                # session 日志归档
│   └── YYYY/MM/DD/<session-id>.jsonl.gz.enc   # gz + KMS encrypted
├── rules/                               # CLAUDE.md 同步
│   ├── current/
│   │   ├── CLAUDE.md
│   │   ├── data-warehouse.md
│   │   ├── doris-credentials.md
│   │   ├── environment.md
│   │   ├── event-protocol.md
│   │   └── java-flink.md
│   └── history/
│       └── YYYY-MM-DD-HHmm/             # 每次修改自动归档
├── manifests/                           # 私有 manifest 加密同步
│   └── <device>/<profile>.yaml.enc
└── snapshots/                           # 重要 snapshot 上传备份
    └── <device>/<ts>.tar.gz.enc
```

#### 7.4.4 状态文件（本地 `~/.frank/state.json`）

```json
{
  "schema_version": 1,
  "device_id": "ATHENA-LAPTOP",
  "last_sync_at": "2026-05-21T17:40:00+08:00",
  "active_profile": "personal",
  "installed_skills": {
    "doris-ops": {
      "version": "abc123def",
      "installed_at": "2026-05-21T10:00:00+08:00",
      "enabled": true,
      "platforms": ["claude", "codex", "opencode"],
      "source_ref": "main",
      "last_health_check": "2026-05-21T17:35:00+08:00",
      "health_status": "ok"
    },
    "kdwl:vehicle-events": {
      "version": "94112d8",
      "enabled": true,
      "platforms": ["claude", "codex"],
      "profile": "company"
    }
  },
  "active_mcp_servers": [
    "memory",
    "fetch",
    "sequential-thinking"
  ]
}
```

---

## 8. 安全设计

### 8.1 凭据存储矩阵

| 凭据类型 | 存储位置 | 访问方式 |
|---|---|---|
| Git SSH key | `~/.ssh/id_*` (OS 默认权限) | git 直接读 |
| GitHub PAT | Windows Credential Manager / macOS Keychain | `credentials.go` 调 native API |
| 数据库密码（doris/mysql） | 同上 | 渲染 config.ini 时注入 |
| 腾讯云设备证书 | OS 证书存储 | mTLS 时由 sync-client 取 |
| KMS 主密钥 | 不下载，云端管理 | KMS API decrypt at-use |

**红线**：
- ❌ **禁止**任何凭据明文落 YAML
- ❌ **禁止** credentials.ini 入 git（pre-commit hook 扫描）
- ❌ **禁止** sync 上传未加密的凭据字段

### 8.2 权限隔离三层

1. **manifest 分仓**：private skills 的 manifest 不进公开 repo
2. **git 鉴权**：private repo 无 read 权 = clone 直接拒
3. **device-allowlist**：manifest 项指定 hostname，错误设备装不上

### 8.3 公司 skills 红线

| 红线 | 强制手段 |
|---|---|
| 严禁 push 公司 skills 到公开 repo | pre-commit hook + GitHub Actions 扫描内网 IP (10.0.\*/10.89.\*/10.90.\*) |
| 严禁 manifest 含公司 repo URL 的公开提交 | `.gitignore` + Git hook |
| 严禁公司 skills 内容写入腾讯云 COS（除非 KMS 加密） | sync-client 强制加密 |
| 严禁分发到非 allowlist 设备 | adapter 安装前校验 hostname |

---

## 9. AI 自维护机制

### 9.1 允许的操作（feature 分支）

| 操作 | AI 可做 | 分支 | review 流程 |
|---|---|---|---|
| 改 SKILL.md description 优化 trigger | ✅ | `feature/ai-trigger-<skill>-<date>` | 自动提 PR，待人 merge |
| 加 description keywords | ✅ | 同上 | 同上 |
| 加 example 用例 | ✅ | `feature/ai-example-<skill>` | 同上 |
| 修 bug（脚本级 typo） | ✅ | `feature/ai-fix-<skill>` | 同上 |
| 加 health-check | ✅ | `feature/ai-health-<skill>` | 同上 |

### 9.2 禁止的操作

- ❌ 直接 push main
- ❌ 改 manifest visibility（private → public 风险）
- ❌ 改 auth 字段
- ❌ 删除文件
- ❌ 改 credentials.ini
- ❌ 跨 skill 大规模重构

### 9.3 PR 流程

```
1. AI 在使用中发现 skill 问题
2. frank ai-suggest 命令（或 AI 主动）
   - 在 feature/ai-* 分支上 commit
   - 写 PR description: 改了什么 / 为什么 / 影响范围
   - 触发 GitHub Actions smoke test
3. 人 review PR
   - 看 diff
   - 看 CI 结果
   - 决定 merge / reject
4. merge 后 frank update 拉取新版
```

---

## 10. 演进路线

### P0 — MVP 核心（1 周）

**目标**：CLI 能装/卸/启/禁/列，三平台同步生效。

| Day | 任务 | 验收 |
|---|---|---|
| 1-2 | scaffold + manifest schema + 解析 | `frank list` 能渲染表格 |
| 3-4 | 三 adapter + installer + state | `frank install doris-ops` 三平台都装上 |
| 5 | snapshot + uninstall + tests + CI | `frank uninstall` 干净，smoke test 矩阵过 |

### P1 — 安全更新与回滚（3 天）

| 任务 | 验收 |
|---|---|
| `frank update` + snapshot before | 失败自动 rollback，不污染状态 |
| `frank rollback --to <ts>` | 60 秒内恢复任意历史快照 |
| `frank doctor` | 列出所有 skill 健康状态 |

### P2 — 分布式记忆（v1 表述,实际拆分到 P5 / 部署见 ADR-005)

> ⚠️ 原 P2 (TencentDB + COS + KMS) 已被 ADR-003/005 拆分: 分布式记忆 → P5 (frank-memory + Qdrant); 部署 → 自建 Docker stack tx:8318; CLAUDE.md / session log 同步推迟。下表保留供历史对比。

| 任务 | 验收 |
|---|---|
| sync-agent 部署到腾讯云（CVM + DB + COS + KMS） | HTTPS + mTLS 通 |
| memory MCP 后端切换到 TencentDB | mcp__memory__* 写云端、跨设备读到 |
| CLAUDE.md 上传 / 拉取 | 两台设备 rules 同步 |
| session log 自动归档 | 每日 COS 写入成功 |
| skill 调用统计 + `frank stats` | 数据准确，summary 表更新 |

### P3 — WebUI（1 周）

- Tauri scaffold + 5 个核心页面
- 调用 frank CLI 作为后端
- macOS / Windows 双平台打包

### P4 — AI 自维护（1 周）

- GitHub App 注册（提 PR 用）
- `frank ai-suggest` 命令
- AI prompt 模板（让 AI 知道改什么 / 不改什么）
- Smoke test 矩阵扩展（三平台 + AI 改动 = 9 case）

### P5 — frank-memory: mem0 同思路 Rust 重写（2 周,进行中）

详见 [ADR-003](ADR/003-frank-memory-rust.md)。

| 任务 | 验收 |
|---|---|
| `crates/frank-memory` 骨架: store/embed/extract/client | 🟢 已落地,14 单测全绿 |
| Qdrant 容器在 tx:8318 跑通 | 🟢 已部署 (2026-05-21) |
| OpenAI embedding + Anthropic Haiku fact extractor | 端到端真测 (待 API key) |
| `frank-sync-agent` REST: `/memory/add` `/memory/search` 等 | axum 路由就位,待业务接线 |
| `frank memory add\|search\|list` CLI 子命令 | 调 sync-agent,跨设备生效 |

### P6 — frank-orchestrator: 多 Agent 协作总线（1-2 周,设计完成）

详见 [ADR-004](ADR/004-frank-orchestrator.md)。

| 任务 | 验收 |
|---|---|
| `crates/frank-orchestrator` 骨架 (Job / Step / Worker trait) | 🟢 已落地 |
| Postgres job 表 schema + sqlx | DDL 跑通,job 状态机持久化 |
| `RestWorker` (Claude / OpenAI / Anthropic) + `LocalCliWorker` (codex / gemini) | 单 step 真跑通,日志 WS 推流 |
| 浏览器 Web UI: 任务看板 + 单任务时间线 + WS 实时流 | 静态 SPA,caddy 反代 `/ui/` |
| 与 frank-memory 联动: 跨 job 经验召回 | search 拿到上下文,提示词注入 |

---

## 11. 风险登记

| ID | 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|---|
| R1 | 公司 skills 误泄露到公开 repo | 中 | 严重 | pre-commit hook + CI 扫描 + 分仓 |
| R2 | 三平台 yaml 字段不兼容，adapter 漏转 | 中 | 中 | adapter 单元测试矩阵 + smoke test |
| R3 | 腾讯云 sync-agent 宕机，本地离线不可用 | 低 | 中 | 本地为主，sync 是可选；缓存最近一次 sync 状态 |
| R4 | KMS 主密钥丢失，所有加密数据无法解 | 低 | 极严重 | KMS 备份策略 + 离线纸质备份 |
| R5 | AI 自维护 PR 中投毒（prompt injection） | 中 | 严重 | 人工 review 强制 + CI 静态扫描 + AI 沙箱权限 |
| R6 | junction 在 Windows 跨盘失效 | 中 | 中 | install 前 detect 同盘，否则 fallback copy |
| R7 | git LFS / 大文件 clone 慢 | 中 | 低 | sparse-checkout + subpath |
| R8 | 多设备同时写 state.json 冲突 | 低 | 低 | sync 用 CAS（Compare-And-Swap）+ 设备指纹 |
| R9 | health-check 卡死阻塞 install | 中 | 低 | timeout 强制（默认 10s） |
| R10 | manifest schema 演进破坏旧客户端 | 中 | 中 | schema_version 字段 + 向后兼容策略 |

---

## 12. ADR（架构决策记录）

### ADR-001：选 Rust 实现 frank-cli

- **决策**：CLI 使用 Rust 1.75+ 实现
- **决策时间**：2026-05-21
- **决策者**：hutiefang（用户拍板）

#### 选型背景

frank 要面向**陌生人分发**（非自用），分发体验是核心约束。三语言对比后用户选 Rust。

#### 理由

1. **二进制最小**：1-3 MB，远低于 Go (5-15 MB) 和 Java GraalVM Native (10-30 MB)
2. **冷启动最快**：1-5 ms（CLI 用户感知友好）
3. **npm 分发生态成熟**：biome / swc / esbuild / dprint 已验证 Rust + npm wrapper 模式
4. **包管理器全覆盖**：cargo install / brew / scoop / winget / npm 多渠道分发
5. **AI 协作可接受**：用户明确表态 "AI 写压力不大"，由 AI 主笔承担学习曲线
6. **未来扩展空间大**：sync-agent / WebUI (Tauri) 可统一 Rust 技术栈

#### 代价与缓解

| 代价 | 缓解措施 |
|---|---|
| 学习曲线陡（borrow checker） | AI 主笔，用户 review；严格 `clippy::pedantic` + `forbid(unsafe_code)` 守护 |
| 3 个月后回头改代码 | 全量文档注释（`#![warn(missing_docs)]`）+ 模块化（每文件 < 300 行） |
| AI 写 Rust 易翻车 | 强制单元测试覆盖；CI 三平台矩阵；每模块独立可测 |
| GraalVM/JIT 没有运行时反射 | 不依赖反射的库选型；`serde` 派生序列化 |
| git2/libgit2 系统依赖 | `git2` crate 启用 `vendored-libgit2` feature，无系统依赖 |

#### 质量基线（写入项目规范）

用户明确要求三条，落地为 CI 强制：

- ✅ **代码结构清晰**：每文件 < 300 行；每模块单一职责；`clippy::pedantic` 全开
- ✅ **注释完整**：`#![warn(missing_docs)]`，每个 `pub` item 必须有 `///` 文档注释
- ✅ **打印清晰**：用 `tracing` 做结构化日志 + `owo-colors` 做 UI 着色；统一 `log.rs` 模块封装

#### 替代方案

- **Go**：学习曲线低、AI 协作准确率高，但二进制大 5x、缺少 cargo 这样的一站式工具链
- **Java + GraalVM Native**：用户最熟，但跨平台编译必须各平台跑 GraalVM（不能像 Rust 一行 cross-compile），分发体验差
- **Java + jpackage**：包体 80-150MB（含 JRE），陌生人安装体验差
- **Python + uv**：原型快，但分发需用户装 Python 或用 PyOxidizer（生态不如 Rust）

### ADR-002：manifest-driven 而非 hardcode

- **决策**：所有 skill / MCP 元数据用 YAML manifest 描述
- **理由**：复用 kdwl 已验证的模式；让加新 skill = 改 YAML，不改代码
- **替代方案**：硬编码列表（每加一个就要发版）

### ADR-003：git + 腾讯云 sync-agent 混合架构

- **决策**：skill 源码用 git 分发，状态/记忆走腾讯云
- **理由**：
  - git 已是事实标准，免维护服务端
  - 但 git 不适合存运行时状态（频繁小写）和 memory（要 query）
  - 两者职责分离
- **替代方案**：纯 git（无 memory/stats） · 纯 server（自己存代码维护成本高）

### ADR-004：三平台用 junction/symlink 而非各自维护副本

- **决策**：skill 源码在 frank 自己的 cache 目录，三平台用 symlink/junction 指过去
- **理由**：单一来源；更新一处三平台立刻生效；磁盘占用低
- **代价**：Windows junction 跨盘限制
- **替代方案**：copy（升级要复制三份）

### ADR-005：公司 skills 用独立 private manifest

- **决策**：公开仓只放 public manifest；private manifest 在本地 + 腾讯云加密同步
- **理由**：杜绝公开 repo 泄露公司信息（kdwl 已踩过坑）
- **替代方案**：单 manifest 含全部（必然泄露 URL）

### ADR-006：AI 自治权限设为 feature 分支可写

- **决策**：AI 可 push 到 `feature/ai-*` 分支，不可 push main
- **理由**：平衡自动化与可控；保留人 review 节点
- **替代方案**：完全只读（自动化潜力浪费） · 完全自治（风险过大）

### ADR-007：用 Tauri 而非 Electron 做 WebUI

- **决策**：WebUI 用 Tauri (Rust)
- **理由**：包体小（~10MB vs Electron ~150MB）；启动快；与 frank 主项目共享生态（虽然 frank-cli 是 Go，但 Tauri 不需要主项目语言一致）
- **代价**：Tauri 生态比 Electron 小
- **替代方案**：Electron · 纯 Web（无 OS 集成）

---

## 13. 开放问题

需要后续讨论的事项：

| ID | 问题 | 优先级 |
|---|---|---|
| Q1 | 公司 GitLab 是否要支持（除了 GitHub）？kdwl 同时挂 GitHub + GitLab | P0 前定 |
| Q2 | 是否需要 Linux 桌面支持，还是只 Win + Mac？ | P3 前定 |
| Q3 | sync-agent 是否要支持自部署（让其他人用 frank 但用自己的腾讯云）？ | P4 前定 |
| Q4 | mcp server 启动管理是否纳入 frank（process supervision）？还是依赖各 CLI 自带的 mcp 管理？ | P0 前定 |
| Q5 | 是否要支持 skill 私享给团队成员（小范围共享 private skill）？ | P4 后 |
| Q6 | session log 是否要本地脱敏后再上传？（避免 KMS 出问题时还能多一层） | P2 前定 |

---

## 14. 附录

### 14.1 参考资料

- Claude Code skills 规范：https://docs.claude.com/claude-code/skills
- Anthropic Skill Engineering: https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills
- Tauri 文档：https://tauri.app
- clap (Rust CLI 框架):https://docs.rs/clap
- Qdrant Rust SDK:https://github.com/qdrant/rust-client
- mem0 (思路参考):https://github.com/mem0ai/mem0

### 14.2 相关项目（你已有的）

- `skills-kdwl` (private) — 公司 skills 集合，frank 的 private manifest 主要来源
- `~/.claude/CLAUDE.md` + `~/.claude/rules/*.md` — 待同步的规则
- 各开源 skill repos（doris-ops / feishu-read / nacos-config 等）

### 14.3 词汇表

| 简称 | 全称 |
|---|---|
| ADR | Architecture Decision Record |
| CAS | Compare-And-Swap |
| KMS | Key Management Service |
| MCP | Model Context Protocol |
| mTLS | Mutual TLS |
| PAT | Personal Access Token |
| RBAC | Role-Based Access Control |
| TUI | Terminal User Interface |

---

## 15. 文档版本演进

| 版本 | 日期 | 作者 | 变更 |
|---|---|---|---|
| 0.1 | 2026-05-21 | hutiefang + Claude | 初始 draft,对齐 P0-P4 架构 |
| 0.2 | 2026-05-21 (P5/P6 启动) | hutiefang + Claude | 见下条目 |

**0.2 — 2026-05-21 (P5/P6 启动)**

- mem0 路线由 Python 服务改 Rust 重写 ([ADR-003](ADR/003-frank-memory-rust.md))
- 多 Agent 协作由 CCB tmux 改 Web UI + API ([ADR-004](ADR/004-frank-orchestrator.md))
- 部署到 tx:8318 ([ADR-005](ADR/005-deploy-tencent-8317.md))
- 仓库结构改 Cargo workspace ([ADR-002](ADR/002-cargo-workspace.md))
