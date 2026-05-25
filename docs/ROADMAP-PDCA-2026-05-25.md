# ROADMAP-PDCA — 用户 18 条需求剩余 14 条 4 阶段交付

| Field | Value |
|---|---|
| **Created** | 2026-05-25 (用户睡醒后追责 → 立 PDCA 框架) |
| **Owner** | hutiefang |
| **Method** | 4 阶段 × PDCA (Plan / Do / Check / Act) + 多 agent 并行赶工 |
| **Decision authority** | 用户(每阶段 Plan 完成需用户 go/no-go) |
| **Quality gate** | 测试 + clippy + fmt + audit + secret-scan + **端到端真测 + brew + tx 真部署** |
| **No-codex policy** | 不用 codex 审批 (用户明确说"还有 bug")—— 改用 general-purpose / workflow agent 做研究,质量靠**测试 + 真测**保证 |

## 4 阶段路标

| Phase | 版本 | 工期 | Task IDs | 主轴 |
|---|---|---|---|---|
| **Phase 1** | v0.10.5 | 1d | #78 | **可观测性** — token/cost/latency/source 全打 |
| **Phase 2** | v0.10.6 | 1d | #79 | **引导** — doctor 全景 + CLAUDE.md 模板 |
| **Phase 3** | v0.11.0 | 7-10d | #80-#85 (6 子任务) | **本地缓存 + 多路召回 + 多设备** |
| **Phase 4** | v0.12.0 | 10-15d | #86-#90 (5 子任务) | **差异化王炸 + 多层级 + procedural + MCP** |

每阶段 ship gate 必须含: brew upgrade 真测 + tx sync-agent 同步 (若涉及) + 端到端真跑通。

---

## PDCA 模板 (每阶段必走完整循环)

### P (Plan) — 详细设计 + 可行性分析

**入口**: 阶段开始,**必须**先做这一步,不可跳。

**动作**:
1. 写 `docs/phases/PHASE-{N}-PLAN.md` 草稿(基于已有 ADR-007/ADR-009 + 当前用户需求)
2. **Spawn design+feasibility agent(s)** — 多任务并行:
   - 调研技术选型(必要时 WebSearch / context7)
   - 评估可行性(risk / blockers / unknowns)
   - 输出"go/no-go"推荐 + 反对意见
3. 我综合 agent 输出 → 修订 Plan → 拆 sub-tasks
4. **决策点**: 用户 confirm 才进 D 阶段

**ship gate**:
- [ ] Plan 文档存在且 < 300 行
- [ ] 可行性分析报告附在 Plan 后(包含 known unknowns)
- [ ] sub-tasks 全拆好进 TaskList
- [ ] **用户明确 go**(AskUserQuestion)

### D (Do) — 多 agent 并行执行

**动作**:
1. 按 Plan 拆解的 sub-tasks 启动 N 个 worker agent(独立 git worktree)
2. 每个 worker 专注一个 sub-task,跑完 → commit 到 worktree
3. 我做 orchestrator:监控 + 集成 + merge 冲突解决
4. 单 agent 阶段(v0.10.5/v0.10.6 小)直接我自己跑

**ship gate**:
- [ ] 所有 sub-task 实施完
- [ ] 每个 worker 自测过(workspace test pass)
- [ ] 集成后 main 编译 + workspace test 全绿

### C (Check) — 测试 + 验证

**动作**:
1. cargo test --workspace --all-features
2. cargo clippy --workspace --all-targets --all-features -- -D warnings
3. cargo fmt --all -- --check
4. cargo doc --no-deps --all-features (RUSTDOCFLAGS=-D warnings)
5. CI secret-scan 模拟
6. **端到端真测**:`./target/release/frank <new-feature> ...` 实跑通
7. 必要时跑 verification agent(workflow-verifier)三层验证

**ship gate**:
- [ ] 全测试绿
- [ ] 端到端真测产物贴出来(命令 + 输出)
- [ ] 已知未实现的 known gap 列入下一阶段 task

### A (Act) — Ship + 闭环 + 教训

**动作**:
1. bump Cargo.toml workspace.package.version
2. git commit + tag + push
3. 等 3 个 workflow (ci/release/sync-agent-image) 全 success
4. 拉 4 sha256 → bump Homebrew Formula → push
5. **`brew upgrade frank` 真装 + 4 项端到端真测**
6. (若改 sync-agent) SSH tx + docker compose pull + restart + health 200
7. release note 写
8. **教训记录**: `docs/lessons/PHASE-{N}-LESSONS.md`(避免下次重蹈)

**ship gate**:
- [ ] Release page 12 asset 齐全
- [ ] brew 实地装新版 + 真测 4 项 / 5 项全过
- [ ] (若改 sync-agent) tx health 200
- [ ] release note 发布
- [ ] 教训记录文档存在

---

## 多 agent 赶工策略

| 阶段大小 | 并行度 | 工具 |
|---|---|---|
| 小阶段 (v0.10.5/v0.10.6, 1 子任务) | 1 design agent + 我自实施 | general-purpose / Explore |
| 中阶段 (Phase 3, 6 子任务) | 3-4 worker agent 并行 | workflow-executor (worktree isolation) |
| 大阶段 (Phase 4, 5 子任务) | 3-4 worker agent 并行 | workflow-executor + workflow-verifier |

**Worktree 隔离**: 用 `Agent(isolation: "worktree")` 让 worker 各自 fork main 跑,完成后我 merge。

**集成 cadence**: 每个 worker 完成 → 我 review diff → merge 进 main → 跑 workspace test 验证。

---

## 决策记录 (replace codex Plan Review/Code Review)

- 每阶段 P → 决策(我推荐 + 用户拍板)记 `docs/decisions/{phase}-{topic}.md`
- 每阶段 A → 教训记 `docs/lessons/PHASE-{N}-LESSONS.md`
- 不再走 codex 评分(用户明确说"还有 bug")—— 改用 **测试 + brew 真测 + 用户验收** 三道闸

---

## 当前进度

- [x] **v0.10.4 已 ship** (ADR-009 凭据桥, 18 条需求里 1.5 条)
- [ ] **Phase 1 (v0.10.5) — P 阶段 in_progress**(本文创建时启动)
- [ ] Phase 2 / 3 / 4 pending

---

## 看板入口

- 完整 task: `TaskList` #78-#90
- 阶段 plan: `docs/phases/PHASE-N-PLAN.md`
- 决策: `docs/decisions/`
- 教训: `docs/lessons/`
- 18 条需求归档: 本文 + 用户原话粘贴留底
