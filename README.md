# Frank

> AI 工具链治理平台 — 统一管理 Claude Code / codex / opencode 三平台的 skills 与 MCP。

[![Status](https://img.shields.io/badge/status-WIP-orange)]() [![License](https://img.shields.io/badge/license-MIT-blue)]()

## 是什么

Frank 是一个 CLI 工具，帮你在多台设备 + 多个 AI CLI 之间统一管理：

- **skills**(见下方 "Skills 两层 5 档分类")
- **MCP servers**
- **CLAUDE.md 规则同步**
- **分布式记忆 + 调用统计**(自建 Docker stack: qdrant + caddy + frank-sync-agent,跑在腾讯云 VM)

### Skills 两层 5 档分类

frank 治理两种来源的 skills:

**Layer 1 — frank 内置(项目作者维护,装 frank 默认就有):**

| visibility | 含义 | 维护者 |
|---|---|---|
| `frank-own` | **芳哥自研** — frank 项目作者 (hutiefang76) 自己写的开源 skills | 项目作者 |
| `frank-recommended` | **芳哥推荐** — 项目作者推荐的 upstream / 第三方 skills (如 anthropics/*) | 项目作者列名单 |

→ 装 frank 立刻就有这两类(`frank list` 默认展示),一键 install。

**Layer 2 — 用户自定义(用户自己 manifest 加,跟项目作者无关):**

| visibility | 含义 |
|---|---|
| `user-public` | 用户自己开源的 skills (公开 git URL) |
| `user-company` | 用户**自己公司**的 skills (跟 frank 项目方无关,严禁混入本仓!放 `~/.frank/manifests/`) |
| `user-private` | 用户自己机密的 skills (个人凭据) |

→ 放在 `~/.frank/manifests/*.yaml`,frank 启动时自动合并。

> v0.1 老 `public` / `own-public` / `private` 通过 serde alias 兼容,不破老配置。

## 核心特性 (v0.13.2)

**Skill / MCP 管理**
- 🚀 一键 install/uninstall/enable/disable skills, 三平台同步 (Claude Code / codex / opencode)
- 📦 MCP 真状态探活 — 看 `~/.claude.json` / `~/.codex/config.toml` 等真 config 文件, 不只信 state
- 🔒 内置 skill 卸载保护 — `frank-ask-*` / `frank-mem-*` 卸了 frank 残废, 默认拦, 要 `--force-internal` 才放
- ⚡ 装前 preflight 检查 — `frank install frank-ask-gemini` 先 `which gemini`, 没装就警告

**分布式记忆 (mem0 Rust 重写)**
- 🧠 Hybrid Retrieval 3 路并行召回 + RRF 融合 (ADR-011, Cormack 2009)
- 🎯 LanceDB 本地主存 + Qdrant 服务端 (ADR-010, 嵌入式向量库, <50ms 命中)
- 🔄 LLM 事实抽取 (claude/codex/gemini 任一可用, auto fallback) — 默认 0 token (LocalEmbedder fastembed 384d)
- 🌐 跨设备同步: `frank-sync-agent` (Rust axum REST), 自托管或 `frank.hutiefang.com`

**跨 AI ask + 凭据管理**
- 🤖 `frank ai ask --claude/--gpt/--opencode/--gemini "..."` 一行调任意 cli (4 家)
- 🔑 5 层凭据桥 (ADR-009): keychain → env → config → file → OAuth session (不注 env, 避免 401)
- 📊 history 自动落 + 跨 session 上下文注入 (`--context-from <tag>`)

**Tenant + 防 spam (v0.13.0+)**
- 🆔 服务端发 token — 客户端首次跑提交机器指纹 (hostname/MAC/CPU/OS), `machine_code` 1:1 绑 tenant (ADR-013)
- 📊 quota 10k records/tenant, `frank tenant status` 查用量
- 🗑️ 14 天删除流程 — `frank tenant delete` 倒计时, `cancel-delete` 撤回, 真到点 qdrant points delete

**自建友好**
- 🐳 一键自建 server — `curl ... install-server.sh | bash`, docker compose 起 caddy + qdrant + sync-agent
- 📦 docker image 80MB (v0.10.10 起瘦身, 模型走 volume mount)
- 🌍 用户隔离 — `X-Frank-Token` sha256 → tenant_id, 服务端注入 scope, 数据互不可见

**三种交互**
- 🎛️ CLI (主) / Web UI (`frank ui`) / Slash command (`/frank:ask:claude`)

## 快速上手

### 安装

**macOS / Linux — Homebrew (推荐, v0.5.1+)**

```bash
brew install hutiefang76/frank/frank
```

第一次会自动 `brew tap`, 以后 `brew upgrade frank` 自动升最新版。
跨架构全覆盖: macOS arm64 / x86_64 + Linux arm64 / x86_64。tap 源: <https://github.com/hutiefang76/homebrew-frank>。

**全平台 — 一键脚本** (含 Windows 自动选 .zip)

```bash
curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/install.sh | bash
```

自动选最快路径:先下预编译 binary,失败再 `cargo build`。

**手动 — 从 Release 下 archive** (无网络脚本 / 离线机器友好)

到 [Releases](https://github.com/hutiefang76/skills-frank/releases/latest) 下对应平台:

| 平台 | archive |
|---|---|
| macOS (Apple Silicon) | `frank-v<X.Y.Z>-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `frank-v<X.Y.Z>-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 | `frank-v<X.Y.Z>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `frank-v<X.Y.Z>-aarch64-unknown-linux-gnu.tar.gz` |
| Windows x86_64 | `frank-v<X.Y.Z>-x86_64-pc-windows-msvc.zip` |
| Windows aarch64 | `frank-v<X.Y.Z>-aarch64-pc-windows-msvc.zip` |

例 (macOS Apple Silicon, v0.5.1):
```bash
curl -fsSL https://github.com/hutiefang76/skills-frank/releases/latest/download/frank-v0.5.1-aarch64-apple-darwin.tar.gz | tar xz
sudo install -m 755 frank /usr/local/bin/    # 或: mv frank ~/.local/bin/
frank doctor                                 # 验证安装
```

**仅开发者** — 源码 build (修代码 / 跑测试)

```bash
git clone https://github.com/hutiefang76/skills-frank.git
cd skills-frank
cargo install --path crates/frank-cli --locked    # 全局装
# 或 cargo run -- doctor                          # 不装直接跑
```

> ⚠️ `cargo install frank` **永久不可用** — crates.io 上 `frank` 这个名字早在 2019 年被别人占了 (跟本项目无关)。发布到 crates.io 需要改名 (如 `frank-cli`),P1 决定。
>
> ⚠️ `npm i -g @hutiefang/frank-cli` 也不可用 — npm wrapper 留到 P1。

### 验证安装

```bash
frank doctor          # 11 项环境健康检查 (toolchain / ~/.frank/ / 三平台目录 / sync-agent)
frank scan            # 扫本机三平台 skills 目录
frank list            # 列 manifest 里的 skills
```

### 配置 sync-agent token (v0.5.1, `frank login`)

sync-agent 在 Caddy 层用 `X-Frank-Token` header 守 `/memory/*` `/orchestrator/*`,
没 token 直接 401。**装完 frank 跑 `frank login` 一次,以后所有 memory / orchestrator
命令都自动带 token**:

```bash
frank login --from-host tx        # ssh 拉 /opt/frank/.env 里的 FRANK_API_TOKEN, 写本机 ~/.frank/.token (600 权限)
frank login --token <xxx>         # 手敲 token (适合 1Password / 团队分发)
frank login --show                # 看当前 token (脱敏: 前 4 + 后 4)
frank logout                      # 删本机 token
```

> **注意**: `X-Frank-Token` 是访问**你公网 sync-agent** 的鉴权票据,**跟 LLM 计费 token
> 完全无关**。`frank memory list/search` 只读 qdrant 不调 LLM,**零计费**;
> 调 LLM 的只有 `frank ai ask` 和 `frank memory add` (后者要 LLM 抽 fact)。

### daemon (可选) — 仅 Web UI / 多 agent 共享上下文用户需要

> ⚠️ **frank 90% 功能在 CLI** — `install/uninstall/scan/cleanup/login/orchestrator demo/ai ask` 都不需要 daemon。
> 装完 frank 不主动起 daemon 也能用全部 CLI 命令。**不用 Web UI 就别让它跑**:
> ```bash
> brew services stop frank   # 关掉 daemon, CLI 不受影响
> ```

daemon 的实际价值:
- ✅ **Web UI** (`http://127.0.0.1:7780`): 给"想要图形界面"的人。下拉选 cli + 输 prompt + 流式回显
- ✅ **共享上下文** (v0.8+): claude 写代码 → codex review 时自动拿到 claude 的上下文 (跨 cli 共享 memory)
- ⏳ ~~多设备分布式 memory~~ / ~~多 agent 自动协作~~: 设计中,未实现

**brew 装的用法** (推荐):
```bash
brew install hutiefang76/frank/frank      # 装 binary, 不自动起 daemon
brew services start frank                  # 想要 Web UI 才主动起 (一次, 重启自动)
open http://127.0.0.1:7780                 # 看 Web UI
brew services stop frank                   # 关掉
brew services list | grep frank           # 看状态
```

**源码 / 一键脚本装的用法** (无 brew 时):
```bash
# 装一次 (写 ~/Library/LaunchAgents/com.frank.orchestrator.plist + 立刻起)
frank daemon install              # 默认 127.0.0.1:7780
frank daemon install --port 7799  # 自定义端口

# 日常
frank                             # 无参: 自动开浏览器到 daemon URL
frank daemon status               # 看 PID + 端口
frank daemon stop / start
frank daemon uninstall            # 移除 launchd 注册 + 删 plist

# 日志
tail -f ~/.frank/logs/orchestrator.out.log
```

> brew 装的环境下 `frank daemon install/start/stop` 写操作被禁用 (v0.7+) — 统一走 `brew services`。
> **Linux / Windows**: v0.5 仅 macOS launchd 真接, systemd user unit + Windows 服务留后续。
> 临时跑可用 `frank orchestrator serve --bind 127.0.0.1:7780` (阻塞终端)。

### 安装 skill

```bash
frank install doris-ops               # 公开 skill (manifest/public.yaml)
frank install kdwl:vehicle-events     # 公司 skill (~/.frank/manifests/, SSH + VPN)
frank install --url https://github.com/foo/skills-bar.git   # 任意 git URL (v0.7+)
```

### 缓存机制 (`~/.frank/cache/`)

frank `install` 不复制 skill 文件,而是 **git clone 到本机 cache** + **symlink 到三平台 skills 目录**。这样:

- 升级是增量 `git fetch + checkout`,**不重新 clone** — 几 KB 数据,毫秒级
- 链接不断 — 文件原地变,symlink 自动指向新版本
- 跨设备 cache key 一致(`sha256(url)[..8]` 16 hex),将来分布式同步只算一份

```text
~/.frank/cache/
├── efb0d2a9d0eab3e7/   ← sha256(https://github.com/foo/bar.git)[..8]
│   ├── .git/
│   └── <repo files...>
└── 230b59f412650c3c/
    └── ...
```

| 命令 | 干嘛 |
|------|------|
| 自然占用 | 一个 skill 约 50-200 KB,十几个全装齐 ~1-2 MB,**可忽略** |
| `frank uninstall <name>` | **不删** cache(留着重装时复用) |
| `frank uninstall` (无参) | 默认**删** cache(既然全部卸了,留着没意义) |
| `frank uninstall --keep-cache` | 保留 cache(准备一会重装,或者升级中途回滚) |
| `rm -rf ~/.frank/cache/` | 暴力清,下次 `install` 自动重 clone |

### 卸载 skill

```bash
frank uninstall nacos-ops             # 单卸一个,任何 visibility 都行
frank uninstall                       # 清 frank 自家装的全部 (frank-official + frank-recommended) + cache
frank cleanup                         # 等价 + 打印 brew 卸载引导
```

`frank uninstall` 5 种用法对照:

| 命令 | 干嘛 | 第三方 (`--url` 装的) | cache |
|------|------|------------------|-------|
| `frank uninstall` | 清 frank 自家装的全部 | **保留** | **删** |
| `frank uninstall <name>` | 单卸,任何 visibility | (单卸不影响) | (单卸保留) |
| `frank cleanup` | 同 `frank uninstall` 无参 + 引导 brew 卸载 | **保留** | **删** |
| `frank uninstall --including-3rd-party` | 真全清 | **也清** | **删** |
| `frank uninstall --keep-cache` | 卸 skill 留 cache | 保留 | **保留** |

**设计原则**: frank 只对自己装的负责。用户 `frank install --url ...` 装的第三方 skill,frank 觉得"是你自己挑的",默认不动 — 你装的你自己卸,要求自己清就 `--including-3rd-party`。

### orchestrator 子命令 (P6 M1 真接本机 CLI subprocess)

`frank orchestrator` 真起本机 `claude` / `codex` / `opencode` / `gemini` 子进程,
**走你已付费的订阅(Pro/Plus/Go)**,不重复花 API key 的钱。多 Job 各自 subprocess,
**OS pid 级隔离, 多任务天然不串**。

```bash
# 看本机装了哪些 CLI
frank orchestrator providers

# 真跑一次 codex (gpt-5.5 Plus 订阅)
frank orchestrator demo --provider codex --prompt "Say hi" --timeout 300

# 真跑一次 claude (需要先 claude setup-token 登录 CLI, 见故障排查)
frank orchestrator demo --provider claude --prompt "Say hi"
```

#### 故障排查

| 现象 | 原因 | 修复 |
|---|---|---|
| `claude` exit 1 / 401 auth error | 你机器只登录了 Claude Desktop App,**`claude` CLI 自己没登录** (`~/.claude/.credentials*` 不存在) | `claude setup-token` 一次性登录 CLI(订阅自动识别) |
| `claude` 走 API key 而不是 OAuth | env 里有 `ANTHROPIC_API_KEY` 但是空值 (会覆盖 OAuth) | `unset ANTHROPIC_API_KEY` 然后再跑 |
| `codex` timeout | high-reasoning 慢,默认 120s 不够 | `--timeout 300` 或更高 |
| `frank orchestrator providers` 显示 ✗ | CLI 没装在 PATH 里 | `brew install` / `npm i -g` 对应 CLI |

### 卸载 (v0.7.2+)

⚠️ **重要**: Homebrew 设计哲学是 `brew uninstall` **不动用户数据**(跟 ollama / postgres / redis 一致 — 升降级反复跑不能丢用户数据)。所以 frank 的"用户数据"(三平台 skill symlink、`~/.claude.json mcpServers` 注入、`~/.frank/`)brew 都不会自动清。

#### 一键卸载脚本(推荐)

```bash
# 远程一行 (走代理也 work, 5 个步骤交互式确认)
curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/scripts/uninstall-frank.sh | bash

# 或本地跑
git clone https://github.com/hutiefang76/skills-frank.git /tmp/sf && bash /tmp/sf/scripts/uninstall-frank.sh

# 全自动 yes (不交互)
curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/scripts/uninstall-frank.sh | bash -s -- --yes

# 保留 ~/.frank/ (token + state, 以便重装直接接管)
bash scripts/uninstall-frank.sh --keep-config
```

脚本干 4 件事(每步交互式 y/N 确认):
1. `frank cleanup` — 清三平台 skill symlink + MCP 注入 (`~/.claude.json mcpServers` + `~/.codex/config.toml mcp_servers`) + git cache (`~/.frank/cache/`)
2. `brew services stop frank` — 停 launchd daemon
3. `brew uninstall frank` — 删 binary (brew 自动 untap 跟自动 stop service)
4. `rm -rf ~/.frank/` — 清 token / state / logs / ai_history (可选)

#### 手动卸载

```bash
frank cleanup                      # frank 自家的事 (skill+MCP+cache)
brew services stop frank           # 停服务
brew uninstall frank               # 删 binary
rm -rf ~/.frank/                   # (可选) 用户数据
brew untap hutiefang76/frank       # (可选) 删 tap 注册
```

#### 为什么 `brew uninstall` 不自动调 `frank cleanup`?

Homebrew Formula API **没有 uninstall hook**(只有 `def caveats` 提示)。设计上 brew 只管 binary,用户数据不动 — 这样 `brew install ↔ brew uninstall` 反复跑(升级降级)不丢数据。所以 frank 提供 `frank cleanup` + 上面的 `uninstall-frank.sh` 让用户**主动**触发完整清理,符合 brew 习惯。

### memory 子命令(P5, v0.8 真模式)

frank memory 走分布式向量检索: 语义召回, 跨设备共享。客户端 → sync-agent (远程或本地) → qdrant。

```bash
# 写入一条 raw fact (跳过 LLM 抽)
frank memory add-raw "user prefers vim over emacs" --user alice

# v0.8 新: --extract-with claude/codex 调本机 cli 把长文本抽成多条独立 fact 再存
# 零额外 token 费 — 复用用户已登录 cli 订阅
frank memory add "I switched from emacs to vim 3 years ago because of macros" \
    --user alice --extract-with claude
# → 客户端 claude --print 抽出 ["user switched from emacs to vim 3 years ago",
#                              "user values macro support in editors"]
# → 逐条 add_raw 到 qdrant

# 语义检索 (问 editor 能召回 vim 相关)
frank memory search "editor preference" --user alice

# 列出 / 单查 / 删除
frank memory list --user alice
frank memory get <uuid>
frank memory delete <uuid>
```

### frank ai ask 共享上下文 (v0.8 新)

```bash
# 普通 ask (不注入上下文, 跟 v0.7 一致)
frank ai ask --to codex "implement quicksort in Rust"

# 加 --context-from <session>: ask 前 search memory top-3 注入 prompt 前缀,
# ask 后异步存 (Q+A) 进 memory. 同 session 的 claude/codex/gemini 跨 agent 共享
frank ai ask --to codex --context-from default "review 刚才的实现"
# → frank 自动: search "review 刚才的实现" → 找到刚刚 claude 写的代码 → 拼到 prompt 前
# → codex 看得到上下文, 能真 review 而不是凭空生成
```

SKILL.md (`/frank:ask:gpt` 等) 已教 claude/codex 识别"刚才/那段/继续"等指代表达时自动加 `--context-from default`。

## 开发

```bash
git clone git@github.com:hutiefang76/skills-frank.git
cd skills-frank
cargo build --release        # ./target/release/frank
cargo test                   # 跑单元 + 集成测试
cargo clippy -- -D warnings  # lint
cargo run -- --help          # 跑 CLI
```

## 文档

- [`docs/DESIGN.md`](docs/DESIGN.md) — 完整设计文档（14 章 + 版本演进）
- [`docs/ADR/`](docs/ADR/) — 架构决策记录
  - [001-language-rust](docs/ADR/001-language-rust.md) · [002-cargo-workspace](docs/ADR/002-cargo-workspace.md) · [003-frank-memory-rust](docs/ADR/003-frank-memory-rust.md) · [004-frank-orchestrator](docs/ADR/004-frank-orchestrator.md) · [005-deploy-tencent-8317](docs/ADR/005-deploy-tencent-8317.md)
- [`deploy/README.md`](deploy/README.md) — 服务端部署 (tx:8318)
- [`CHANGELOG.md`](CHANGELOG.md) — 变更历史（待建）

## 技术栈

- **CLI**: Rust 1.75+, Cargo workspace · clap · serde · tracing · owo-colors · git2
- **frank-memory** (P5): Rust · qdrant-client · OpenAI/Anthropic API · async-trait
- **frank-sync-agent** (P5): Rust · axum · 部署在 tx:8318 (Qdrant + Caddy + axum,Docker Compose)
- **frank-orchestrator** (P6): Rust · axum WS · Postgres · 多 provider Worker trait
- **WebUI**: 静态 SPA / Tauri (P3 / orchestrator UI)

## License

MIT © hutiefang
