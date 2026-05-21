# ADR-004: frank-orchestrator — 多 AI Agent 协作总线 (Web UI + API)

| Field | Value |
|---|---|
| **Status** | Proposed (设计, 实现延后到 P6) |
| **Date** | 2026-05-21 |
| **Decider** | hutiefang |
| **Supersedes** | (无) |

## 背景

frank 用户跨多个 AI provider (Claude / codex / opencode / gemini / droid …) 工作。目前用 [CCB](https://github.com/hutiefang76/claude_code_bridge) (tmux 路线) 串联, 三个核心痛点:

1. **必须在 tmux session 里跑** — 离开终端就 break
2. **不支持多任务** — 一个 session 只能跑一组协作, 同时干俩任务要开第二个 session 手动维护
3. **可视化是终端 pane** — 不是浏览器, 远程不便, 截屏 / 分享差

另一个老方案 [GuDaStudio/skills](https://github.com/GuDaStudio/skills) Python, 久未维护, 无可视化。

## 目标

| ID | 需求 |
|---|---|
| G1 | 脱离 tmux: 后台服务化, 浏览器作为 UI 入口 |
| G2 | 多任务并发: N 个 task 互不干扰, 每个有独立的 worker 池 + 日志流 |
| G3 | 可视化: 浏览器看到 task 状态 / 各 provider 输出实时流 / 任务依赖图 |
| G4 | provider 解耦: 用 Worker trait, 适配 REST (Claude/OpenAI/Anthropic) + 本地 CLI (codex/gemini 等) |
| G5 | 与 frank 体系内聚: 借用 frank-memory 存协作历史; 调用 frank-cli 装 skill |

## 候选

| 方案 | 适合性 | 弃用理由 |
|---|---|---|
| A. **frank 原生 axum + WebSocket + 静态 web 前端 (推荐)** | 5/5 | — |
| B. 接 MCP 协议, 让每个 provider 是 MCP server, orchestrator 是多路客户端 | 4/5 | Rust MCP SDK 还嫩 (2026Q2), provider 自身的 MCP 暴露层不齐, 提前抢标准成本高 |
| C. 沿用 CCB tmux 路线, Rust 重写 + 多 session 调度 | 2/5 | 仍受 tmux 环境约束; "终端 UX" 不是用户要的 (要 Web) |
| D. 接 LangGraph / AutoGen 这类成品 | 2/5 | 都是 Python; 拉一坨依赖, 不可控 |

## 决策

**采用方案 A** — frank 自己造总线, 模式如下:

```
┌─────────────────────────────────────────────────────┐
│  浏览器 (任意设备)                                    │
│  - 任务看板 (Kanban)                                  │
│  - 单任务详情: provider 流 / 时间线 / 依赖图           │
│  - 历史回放                                          │
└──────────────────┬──────────────────────────────────┘
                   │ HTTPS + WSS
                   ↓
┌─────────────────────────────────────────────────────┐
│  frank-orchestrator (axum) :8317/orchestrator       │
│                                                     │
│  REST: POST /tasks, GET /tasks/:id, POST /tasks/:id/:action │
│  WS:   /tasks/:id/stream  (sub-protocol: 事件流)     │
│                                                     │
│  调度核心:                                            │
│  - Job (任务): 状态机 (pending → running → done/fail) │
│  - Step: Job 的一步, 含 provider + prompt + 期望产出  │
│  - Worker: 实际跑 step 的实例 (REST 客户端 / 本地 CLI 子进程) │
│                                                     │
│  数据: Postgres (job 历史) + frank-memory (跨 job 记忆)│
└──────────────────┬──────────────────────────────────┘
                   │
       ┌───────────┼───────────┬─────────────┐
       ↓           ↓           ↓             ↓
   ┌────────┐ ┌────────┐ ┌────────┐    ┌──────────┐
   │ Claude │ │ OpenAI │ │ Anthropic│   │ Local CLI │
   │ Worker │ │ Worker │ │ Worker │    │ Wrapper   │
   │ (REST) │ │ (REST) │ │ (REST) │    │ (codex,   │
   │        │ │        │ │        │    │  gemini, ..)│
   └────────┘ └────────┘ └────────┘    └──────────┘
```

## 关键概念

### Job (任务)

```rust
pub struct Job {
    pub id: JobId,
    pub title: String,                // 用户起的名字: "刷一遍 frank 的 P1 PR"
    pub created_at: DateTime<Utc>,
    pub status: JobStatus,            // Pending / Running / Done / Failed / Cancelled
    pub steps: Vec<Step>,             // 一个 job 由一系列 step 组成 (DAG)
    pub dependencies: Vec<JobDep>,    // step 之间的依赖关系
    pub workspace: PathBuf,           // 该 job 的工作目录 (cd 到此再跑 worker)
    pub memory_scope: Scope,          // 关联到 frank-memory 的 scope, 跨 job 召回
}
```

### Step

```rust
pub struct Step {
    pub id: StepId,
    pub kind: StepKind,               // Plan / Code / Review / Test / ...
    pub provider: ProviderId,         // claude / codex / openai / ...
    pub prompt: String,
    pub expected_artifacts: Vec<ArtifactSpec>, // 期望产出 (文件 / diff / 评分)
    pub status: StepStatus,
    pub output: Option<StepOutput>,
}
```

### Worker trait

```rust
#[async_trait]
pub trait Worker: Send + Sync {
    fn provider_id(&self) -> &str;
    
    /// 单 step 执行, 把日志流推到 channel
    async fn run(&self, step: &Step, log_tx: mpsc::Sender<LogLine>) -> Result<StepOutput>;
    
    /// 健康检查 (orchestrator 调度前判断)
    async fn health(&self) -> bool;
}
```

实现:
- `RestWorker` — 通用 REST 客户端 (claude / openai / anthropic)
- `LocalCliWorker` — 包裹本地 CLI (codex / gemini / opencode), stdin/stdout
- `MCPWorker` — (后期) MCP 协议端

### 多任务隔离

- 每个 Job 独立的 workspace 目录 (默认 `~/.frank/jobs/<job-id>/`)
- worker 在 workspace 里启动 (cd 切过去), 各 job 互不串
- 日志 / 中间产物 全部进 workspace; 跨 job 的"经验"由 frank-memory 升上去

## 数据持久化

| 数据 | 存储 |
|---|---|
| Job / Step 元数据 | Postgres (`frank_orchestrator` schema) |
| Job 日志流 | 实时 → WebSocket 客户端; 归档 → COS / 本地文件 |
| Job 间记忆 | frank-memory (Qdrant) |
| Worker 配置 (API key 等) | OS keychain + state.json 指针 |

## 与 ADR-001 / 003 / 005 的关系

- ADR-001 (Rust): orchestrator 是 Rust + axum, 不破纪律
- ADR-003 (frank-memory): orchestrator 通过 frank-memory client 写读跨 job 记忆
- ADR-005 (部署 :8317): orchestrator 跟 frank-sync-agent 同一 binary 起, 由 caddy 反代 :8317

## 与 ADR-001 质量基线一致

每个 crate 同样守:
- clippy::pedantic + -D warnings
- missing_docs warn / forbid unsafe
- 每文件 < 300 行 — orchestrator 状态机部分要小心拆 (可能涉及 task DAG / scheduler / executor / web routes 4 个独立模块)

## 风险

| ID | 风险 | 对策 |
|---|---|---|
| R-O1 | Worker subprocess (本地 CLI) hang | step 设 timeout; orchestrator 主动 kill |
| R-O2 | 浏览器 WS 断流 → 客户端看不到中段日志 | 服务端环形日志缓冲 (last N MB), 重连补播 |
| R-O3 | Postgres 容器在 tx 占内存 | 选 `postgres:17-alpine` (~100MB), 限制 shared_buffers |
| R-O4 | 多 job 同时跑挤爆 RAM (tx 只有 3.3G) | 队列控并发, 默认全局 max 2 active job |
| R-O5 | provider API key 泄露 | 全部走 keychain + KMS; orchestrator UI 不显示 key, 只显示掩码 |

## 不在 P6 v1 范围

- ❌ Job DAG 可视化编辑器 (先文字 / YAML 配, UI 看就好)
- ❌ Workflow templates 市场
- ❌ 多用户 / RBAC
- ❌ 接入 OpenAI Swarm / AutoGen 等

## 后续动作

- [ ] crates/frank-orchestrator 骨架 (P6 day1)
- [ ] Web UI: 前端选型 (Svelte / Vue / 纯 vanilla?) — 待 P6 进入再定
- [ ] Postgres schema 设计
- [ ] Worker trait 第一版 + RestWorker 实现 (Claude provider)
- [ ] WS 协议设计 (事件类型枚举)
