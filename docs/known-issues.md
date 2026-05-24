# Known Issues

按发现时间倒序。每条说清问题、影响范围、临时绕过、计划修复版本。

---

## 2026-05-23 · sync-agent docker buildx 在国内 qemu emulation 下网络不稳, 重建 frank-sync-agent:0.8.0 失败

**问题**: tx 上跑的是 `frank-sync-agent:0.1.0` 老镜像 (mock 模式 zero-vector embed → search 永远 no match)。v0.8 改了 `state.rs` 默认走 `LocalEmbedder` (fastembed 384d, 零 token), 但要让 tx 用新行为必须重 build 镜像。

本机 (mac arm64) docker buildx build linux/amd64 cross-platform 受两个问题影响:
1. **qemu emulation 性能差**: cargo build sync-agent (含 fastembed → ort → onnxruntime) 在模拟下需要 15-30min, 内存峰值 ~3GB
2. **网络极不稳**: 阿里云镜像 / rsproxy.cn 在 qemu 下连接 timeout / slow ("Operation too slow. Less than 10 bytes/sec"), 多次重试同样失败
3. **builder stage 缺 C++ 工具链**: fastembed → ort 静态链 `libstdc++` 需要 `g++ + libstdc++-12-dev` (已修 Dockerfile)

**实测 3 次 buildx 尝试**: 都失败 (链接 stdc++ → 网络 timeout → 网络 timeout)。

**影响**:
- v0.8 frank-cli 代码全部就位 (Phase A/B/B'/C-1/C-2/D 全 commit)
- `frank memory add/list/get` 链路通, **但 `frank memory search` 在 tx mock 模式下永远 no match**
- `frank ai ask --context-from default` 也因此召回不到上下文 (会 graceful 降级到不注入, 不影响 ask 本身)
- **本机起 frank-sync-agent (cargo run -p frank-sync-agent) 是 work 的** (默认 LocalEmbedder), 但 frank-cli 默认指 `frank.hutiefang.com` 不是 localhost

**临时绕过** (高级用户):
```bash
# 本机起 sync-agent + qdrant (用 deploy/test-stack/ 加一段 qdrant service 或直接 docker run qdrant):
docker run -d --name local-qdrant -p 6334:6334 -p 6333:6333 qdrant/qdrant:v1.18.0
FRANK_QDRANT_URL=http://localhost:6334 cargo run -p frank-sync-agent --release
# 指向本机 sync-agent (新终端):
FRANK_SYNC_AGENT_URL=http://localhost:3000 frank memory add-raw "..." --user me
FRANK_SYNC_AGENT_URL=http://localhost:3000 frank memory search "..." --user me
```

**计划修复** (v0.8.1):
- 方向 A: 在 tx (linux/amd64 native) 上 cargo build, 不走 qemu emulation, 替换容器内 binary (docker volume mount)
- 方向 B: 用 GitHub Actions ubuntu-latest (amd64 native) 跑 docker build, push 到 ghcr.io, tx pull
- 方向 C: 用 dockerproxy / 自建 cargo mirror 改善 qemu 网络性能
- 推荐 B: CI 化, 后续每次 sync-agent 改动自动 build + push

---

## 2026-05-23 · [已修 v0.9.0] `frank install --url <git>` 硬编码 ref=main, 默认 master 仓库失败

**问题**: `crates/frank-cli/src/cli/install.rs::synthesize_skill_from_url` 把 git ref 硬编码为 `"main"`。如果目标仓库默认分支是 `master` (或其他), libgit2 fetch `main` 不存在的 ref 会失败:
```
ERROR git fetch for skill xxx: locate FETCH_HEAD after fetch:
corrupted loose reference file: FETCH_HEAD; class=Reference (4)
```

**影响**:
- 任何用 `--url` 装默认分支非 `main` 的仓库都失败
- 实测 `frank install --url https://github.com/hutiefang76/skills-nacos-ops.git` 失败 (该 repo 默认是 `master`)

**临时绕过**:
- 用 manifest 装: 走 `frank install nacos-ops` 而不是 `--url`, manifest 里能配 `ref: master`
- 改本机 binary: 手动 patch install.rs `r#ref: "main"` 改 `"master"` 后重 cargo build

**计划修复** (v0.8):
- ~~选项 A: URL fragment 支持~~
- 选项 B: 加 `--ref <ref>` flag: `--url https://.../foo.git --ref master`
- 选项 C: 失败后 fallback 试 `master`: 不优, 静默 fallback 容易掩盖错配
- 实际 v0.9.0 选 **B + URL query string `?ref=xxx`** 双语法 (fragment 留给 #subpath 保持兼容)

**根因**: P0 day 3-4 实现 `--url` 时只考虑 GitHub 默认 main 仓库, 没考虑老仓库默认 master。

**v0.9.0 修复实测**:
```
frank install --url https://github.com/hutiefang76/skills-nacos-ops.git --ref master test-nacos
# → ✓ `test-nacos` installed (2 platforms, 1.5s) — sha c414605
```
等价 query 语法:
```
frank install --url 'https://github.com/hutiefang76/skills-nacos-ops.git?ref=master' test-nacos
```

---

## 2026-05-23 · RTK `ask` 框架依赖 tmux,新机环境无法走 Plan/Code Review

**问题**: 项目 `CLAUDE.md` 引用 `~/.claude/CLAUDE.md`(RTK 全局),里面强制要求"任何 plan 必须送 codex 做 Plan Review、任何 code 改完必须送 codex 做 Code Review"。这条链路靠 `ask <provider>` 命令,而 `ask` 依赖 **tmux session pane** 做 CCB 协议异步分发。

**影响**:
- 新机器 `brew install frank` 后**没装 tmux** → `ask codex` 直接报 `Pane not alive: 3`
- 走同步降级 `codex exec --skip-git-repo-check < plan.txt` 也不稳定 (实测 v0.8 Plan Review 卡 38min 无响应,被迫 kill)
- 结果: Plan Review / Code Review 在某些环境直接断链,违反 RTK 强制流程但又无可执行替代方案

**用户原话** (2026-05-23): "先记录下这件事情。但是实际情况我不想依赖 shell 客户端。"

**根因**:
1. RTK 是 user-level 全局规范 (在 `~/.claude/CLAUDE.md`),`frank` 项目继承了它
2. RTK 的 `ask` 框架走 tmux session pane,有它的工程原因 (能并发、能复用 cli session、能 async),但**强耦合 tmux**
3. `frank` binary 本身不依赖 tmux,但**协作流程**依赖 — 这是个泄漏

**临时绕过**:
- 同步降级: `codex exec --skip-git-repo-check < plan.txt`(慢、不稳)
- 跳过 Review: 单人开发可接受,但放弃了质量门
- 装 tmux: `brew install tmux` 后 RTK 链路回来,但不是 frank 该要求的依赖

**计划修复** (v0.9 候选,**未承诺**):
- 方向 A: frank 自带轻量 review 命令 `frank review plan <file> --by codex` — 内部 `Command::new("codex").arg("exec")` 直接调,不走 RTK/tmux
- 方向 B: 仅文档化 — `frank doctor` 检测 tmux/ask 不可用时,提示"Review 链路降级到 codex exec 同步模式,可能慢"
- 方向 C: 完全脱钩 RTK — 在 `CLAUDE.md` 显式声明"frank 项目不强制 RTK Peer Review,自带 review 工具"

**决策原则** (用户给的方向): **frank 不应依赖 shell 客户端**。任何 frank 流程都要在裸 `brew install frank` 环境下能跑全。tmux / RTK / ask 这些工具用户**可选装**,装了 frank 能用,没装 frank 也能用。

---

<!-- 后续 known issue 在上面 append, 保持时间倒序 -->
