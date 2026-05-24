# Frank — 当前完成度 & 未完成清单

> 最后更新: 2026-05-24 (v0.8.0 已发, v0.8.1 在做 sync-agent 真模式)
>
> "完成" = 在用户的 macOS 14+ arm64 Homebrew 装环境下端到端真跑通过. "代码就位" = git push 了但没真测.

## 1. 已 ship 真跑通 (绿)

| 模块 | 状态 | 备注 |
|------|------|------|
| `frank install/uninstall/scan/cleanup/list/import/enable/disable/dedupe` | ✅ 真跑通 | brew 装的 cli, e2e 真测 |
| 三平台 adapter (claude/codex/opencode skill symlink) | ✅ | macOS 真测; Linux 应该 OK 但没真测 |
| `~/.claude.json` / `~/.codex/config.toml` MCP server 注入 | ✅ | v0.4 |
| `frank login/logout/--show` (sync-agent token) | ✅ | v0.5.1 |
| `frank doctor` (13 项 check, 含 daemon + sync-agent + state drift) | ✅ | v0.8 加 daemon check |
| `frank ai ask --to <provider>` (claude/codex/opencode/gemini subprocess) | ✅ | v0.7 |
| `frank ai ask --context-from <session>` (共享 memory 注入) | ✅ 代码 | search 部分依赖 v0.8.1 真模式 |
| `frank orchestrator providers/demo/serve` (Web UI :7780) | ⚠️ 基础 | 见 §2 |
| `frank market sync/list` (anthropics + MCP registry) | ✅ | v0.7 |
| `frank config get/set-proxy/detect-proxy` (Clash/Surge auto) | ✅ | v0.7 |
| `frank daemon install/uninstall/status` (macOS launchd) | ✅ | v0.5; brew 装的环境强制走 brew services |
| `frank install --url <git>` (manifest 外任意装) | ✅ | v0.7; **bug: ref 硬编码 main**, 见 known-issues |
| Homebrew tap `brew install hutiefang76/frank/frank` | ✅ | v0.5.1+; 4 平台 arm64/x86_64 mac+linux |
| Release CI: tag → 6 平台 build → archive 上传 release page | ✅ | v0.1+; 含 windows |
| skills-nacos-ops e2e: docker compose 起 nacos + push/list/exists/fetch | ✅ 真测 | v0.8 |
| README: 缓存机制 + 5 种 uninstall 用法 | ✅ | v0.8 |

## 2. 代码就位但真用户视角缺东西 (黄)

### Web UI (`http://127.0.0.1:7780`)

当前能做的:
- ✅ 下拉选 cli (claude/codex/opencode/gemini)
- ✅ 输 prompt → 流式回显
- ✅ 看历史 job (history.jsonl)

**没做的**:
- ❌ skill 管理 (装/卸/查 visibility 全在 cli, UI 不能管)
- ❌ memory 浏览 (`/memory/list` UI 没有, 只 CLI)
- ❌ token / proxy 配置 (`~/.frank/.token` / `config.toml` 全手编)
- ❌ 多 session tab 切换 (只单 session)
- ❌ MCP server 管理 UI (currently `~/.claude.json` 手编)
- ❌ 移动适配 (桌面 only)
- ❌ 鉴权 (本机 127.0.0.1 only, 没设访问控制)

**建议**: Web UI 当前是"功能演示"级别, 不是"日常生产用". 90% 用户场景 CLI 完全够, daemon 可关. 详见 README "daemon 是可选的" 章节.

### Windows 跨平台

build 端:
- ✅ `release.yml` 真 build `x86_64-pc-windows-msvc` + `aarch64-pc-windows-msvc`, release page 有 .zip
- ✅ skill 仓库 (nacos-ops/streampark-ops) setup.bat 已有

**没做的**:
- ❌ `install.sh` Windows 用不了 (bash), README 让 Windows 用户手动解压 .zip → 不友好
- ❌ `scripts/uninstall-frank.sh` 同上, Windows 没等价 .bat
- ❌ `frank daemon install` Windows 服务没接 (v0.5 章节口头说 "Windows 服务留 v0.6", 实际 v0.6+ 也没做)
- ❌ Homebrew 不支持 Windows; chocolatey / scoop / winget package 都没做
- ❌ **没在真 Windows 机器测过任何 frank 命令** (CI 编过, 没真跑过 install + symlink + adapter)
  - 风险: Windows symlink 需要 admin 权限 or developer mode, frank 没处理
  - 风险: Windows 路径含空格 / 反斜杠 / 大小写; PathBuf 应该处理但没验证
  - 风险: 三平台目录 `~/.claude/skills` 在 Windows 是 `%USERPROFILE%\.claude\skills`, dirs crate 处理过应该 OK

**建议**: Windows 视为 "best effort". 真要 ship 给 Windows 用户得专项 1-2 周 (装 Windows VM 真测 + Windows symlink 模式 + .bat uninstall + winget formula).

### Linux 跨平台

build 端:
- ✅ `release.yml` 真 build `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu`
- ✅ Homebrew on Linux (Linuxbrew) 应该能用同一个 Formula
- ✅ install.sh bash 兼容

**没做的**:
- ❌ `frank daemon install` Linux 没接 systemd user unit (v0.5 口头说留)
- ❌ apt / yum / pacman package 没做
- ❌ 没在真 Linux 机器测过 e2e

**建议**: Linux 比 Windows 稳, 大部分代码 mac/linux 共享. 主要差: 没 launchd daemon, 没 native package.

### P5 frank-memory 分布式同步

当前:
- ✅ frank-memory crate: 14 单测全绿
- ✅ frank-cli `mem add/add-raw/search/list/get/delete` 子命令真接 sync-agent REST (reqwest blocking)
- ✅ sync-agent: axum REST 路由 + Qdrant store
- ✅ tx 部署: docker-compose (caddy + qdrant + sync-agent), 走 frank.hutiefang.com
- ⚠️ tx 跑 **mock 模式** (zero-vector embedder), search 永远 no-match. **v0.8.1 正在修**

**v0.8.1 真模式打通** (本次, 在做):
- ✅ 改 sync-agent 镜像 base bookworm → trixie (glibc 2.41 for onnxruntime)
- ✅ nobody user HOME + WORKDIR fix (fastembed local_cache 写权限)
- ✅ ghcr.io 镜像走 GH Actions build (绕 mac arm64 qemu)
- ✅ `LocalEmbedder::from_files` 用 fastembed UserDefinedEmbeddingModel offline API (彻底脱钩 HF)
- ✅ 本机预下 hf-cache 439MB → scp tx → docker-compose volume mount
- 🚧 当前 docker pull 拉新镜像 + restart 验证 healthz

剩 v0.9+:
- ❌ frank-cli 客户端**用客户端 LLM 抽事实** (Phase C-2 代码做了, 但实际依赖每个用户机器的 cli)
- ❌ 多设备同步真测 (两台机器 frank memory add → 另一台 search 召回)
- ❌ memory 列表分页 / 删除 UI
- ❌ scope 隔离真测 (--user / --agent / --session)

### P6 frank-orchestrator 多 agent 协作

当前:
- ✅ `orchestrator providers` 检测本机 cli
- ✅ `orchestrator demo --provider X` subprocess 真接
- ✅ Web UI + WebSocket 流式 (M2)
- ✅ Job history.jsonl 持久化

**没做的** (v0.6+ 设计的协作模式):
- ❌ 接力模式 (claude → codex → gemini 串)
- ❌ 投票模式 (3 个 cli 同问取多数)
- ❌ 对辩模式 (claude vs codex 轮流挑刺)
- ❌ postgres 持久化 (现在只 jsonl)
- ❌ 跨设备 job 共享 (只本机)
- ❌ 失败重试 / 超时控制 / 资源限流

v0.8 用户**重新定义**了 P6 = "多 agent 共享上下文" 而不是 "自动协作" — Phase D 已做这一层 (`--context-from`)。

## 3. 杂项缺/未完工

| 项 | 状态 | 影响 |
|----|------|------|
| `frank cache list / clear` 子命令 | ❌ | 用户看 cache 只能 `ls ~/.frank/cache/` 看 sha hash, 不友好 |
| `frank mem add` 客户端 LLM 抽事实 fall-back | ⚠️ 代码就位没真测 | 用户没装 claude cli 时退化到存原文 |
| `frank update` (skill 批量 fetch 最新 commit) | ❌ | 当前要逐个 `frank install --upgrade` |
| `frank rollback` (回滚到上次 commit) | ❌ stub | v0.1 留的占位 |
| skills-streampark-ops e2e 真测 | ❌ | docker streampark + mysql 太重, 装得上没真跑 |
| 真发 v0.8.1 release | 🚧 当前 | sync-agent 通了就发 |
| CI 三 OS matrix 真跑 install (而不只 build) | ❌ | 风险: macOS-only 测 → Linux/Windows 装出问题 |
| 集成测试 (起 docker qdrant + 本机 frank mem 跑全链路) | ❌ | 当前只 unit |
| 中文 README (i18n) | ❌ | 现在 README 中英混杂 |
| 视频 demo / 截图 | ❌ | github 主页只有文字 |
| frank 自己的 Plan/Code Review 不依赖 RTK ask (tmux) | ❌ | docs/known-issues.md 记了, v0.9 候选 |

## 4. Known Issues (详见 docs/known-issues.md)

1. RTK `ask` 框架依赖 tmux, 新机环境 Plan/Code Review 链路废
2. `frank install --url <git>` 硬编码 ref=main, default-master 仓库失败 (e.g. skills-nacos-ops)

## 5. v0.9 候选 (按优先级猜)

1. **v0.8.1 sync-agent 真模式打通** (当前在做)
2. **frank cache list/clear** 子命令 (~80 行 Rust, 用户友好度高)
3. **`--url --ref` flag** (修 v0.7 install 硬编码 main 的 known issue, ~30 行)
4. **frank update** (批量 fetch, ~150 行)
5. **多设备 memory 端到端真测** (开两台 mac 或 mac+linux 验证)
6. **frank-orchestrator 接力模式** (claude → codex → gemini 串, ~300 行)
7. **Web UI: skill 管理 / memory 浏览** (~1000 行 TS/HTML, 大工程)
8. **Windows 真测专项** (1-2 周)
9. **Linux daemon systemd user unit** (~100 行)
10. **i18n 中文 README + 视频 demo** (产品营销)

每条都可以独立小版本发, 不阻塞其它。
