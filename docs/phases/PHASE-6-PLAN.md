# Phase 6 PLAN — v0.10.6 自动接管 mcp_memory lifecycle

| Field | Value |
|---|---|
| Phase | 6 (PDCA Plan 阶段完成) |
| Version | v0.10.6 |
| Estimated | 3 工作日 |
| Agent verdict | **GO-WITH-CAVEATS**(agent `ae4b369d37bf76934`)|
| 关键风险 | **R1: `~/.claude.json` 200K 行 format-preserving edit** — 硬阻塞 |
| 失败降级 | 若 D3 头 4 小时做不出 → 降级为 "documentation-only auto-prompt"(install 强 prompt 用户跑 `frank memory takeover enable`,frank 自己不动配置) |

## 用户需求映射(POSITION.md 独家定位 3)

| 痛点 | 现状 | v0.10.6 目标 |
|---|---|---|
| 安装 frank 但 official mcp_memory 仍在跑,frank-memory 拿不到流量 | doctor 给推荐让用户手动 disable | install/enable 自动 disable + uninstall/disable restore |
| 用户配置文件被破坏 = 全废 | (无保护) | backup 每次操作 + format-preserving edit + fs2 lock |

## 4 provider 矩阵(关键)

| Provider | disable 方式 | 是否原生支持 | Sev |
|---|---|---|---|
| **claude** | **删 entry** + 全量 backup(`disabled` 字段被 Anthropic 关掉了 #17921 not-planned) | ❌ 无 | 🔴 CRIT(200K 行不能搞坏) |
| **codex** | `enabled = false`(原生支持,#16439 已 merge) | ✅ | 🟡 MED |
| **gemini** | **删 entry** + backup(`enabled` 字段还在 proposal #10493) | ❌ 无 | 🟡 MED |
| **opencode** | `"enabled": false`(原生支持) | ✅ | 🟡 MED |

**关键事实**:claude + gemini 只能删 entry。**全量 backup + format-preserving splice 是命门**。

## 数据模型

### Backup 文件(per provider per disable 操作)
```
~/.frank/backups/mcp-claude-20260525T103000Z.json  (mode 0600)
{
  "schema_version": 1, "frank_version": "0.10.6",
  "action": "disable_official_mcp", "provider": "claude",
  "backed_up_at": "2026-05-25T10:30:00Z",
  "config_path": "/Users/x/.claude.json",
  "entry_location": "mcpServers.memory",
  "original_entry": {"command":"npx", "args":["-y","@modelcontextprotocol/server-memory"]},
  "neighbor_keys": ["context7", "time"]   // restore 时 sanity check
}
```

### Index 文件(单一接管状态真源)
```
~/.frank/backups/index.json
{
  "active_takeovers": {
    "claude": "mcp-claude-20260525T103000Z.json",
    "codex": "mcp-codex-20260525T103000Z.json"
  },
  "history": [{"ts": "...", "action": "...", "file": "..."}]
}
```

**Why per-entry not full-file**:`.claude.json` 200K 行,full-copy 每次浪费 + 累积成 GB 级垃圾。Per-entry 备份 < 1KB。

## 模块切分(新)

```
crates/frank-cli/src/mcp_takeover/   NEW (读写分离,跟 mcp_inspect/ 对称)
├── mod.rs           pub fn disable_all() / restore_all() / status()
├── backup.rs        write/load backup + index.json + fs2 lock
├── claude.rs        JSON entry splice (format-preserving byte-span)
├── codex.rs         toml_edit (新增 dep) 或 shell out `codex mcp disable`
├── gemini.rs        JSON entry splice
└── opencode.rs      JSONC patcher OR re-serialize + warn

crates/frank-cli/src/cli/memory/takeover.rs   NEW
└── status / enable / disable 子命令

crates/frank-cli/src/cli/install.rs           MODIFY (行 159 post-install hook)
crates/frank-cli/src/cli/uninstall.rs         MODIFY (行 53 cleanup restore)
crates/frank-cli/src/cli/memory/handlers.rs   MODIFY (加 takeover 分支)
crates/frank-cli/Cargo.toml                   MODIFY (+ toml_edit, fs2)
/tmp/homebrew-frank/Formula/frank.rb          MODIFY (caveats 加一行)
```

## sub-task 拆解

| Sub | 内容 | 工期 | 备注 |
|---|---|---|---|
| **D1** | `backup.rs` — BackupEntry schema + write/load + index.json + fs2 lock + 15 单测 | 0.5d | 基础设施先行 |
| **D2** | `{codex,opencode}.rs` — `enabled=false` 字段切换(codex 用 toml_edit;opencode byte-splice fallback re-serialize) + 10 单测/provider | 0.5d | 容易那 2 个先做 |
| **D3** | `{claude,gemini}.rs` — entry remove + backup,**format-preserving byte-span splice**;**先写 200K fixture invariant test**(diff 仅 entry 区块) + 15 单测 | **1d** | **硬阻塞** — 4 小时做不出 → 降级方案 |
| **D4** | `cli/install.rs:159` + `cli/uninstall.rs:53` + `cli/memory/takeover.rs` 3 子命令 + TTY 检测 + `FRANK_AUTO_TAKEOVER` env + 8 集成测 | 0.5d | 集成 |
| **D5** | Homebrew Formula caveats 加一行 + CHANGELOG + 4 provider × mac/linux 手测矩阵 | 0.5d | 收尾 |

**合计 3d(agent 估,与 POSITION.md v0.10.6 路标一致)**。

## 关键决策点(D3 失败降级)

```
D3 fixture invariant test (4 hours timer):
    ↓
✅ pass — round-trip 200K .claude.json, diff == 仅 entry 区块
    → 继续 D4 D5 全套 ship
    ↓
❌ fail — byte-span splice 太复杂或破坏 history[]
    → 降级 v0.10.6 为 "documentation-only auto-prompt":
       - install 后强 prompt: "frank doctor 检测到 official mcp_memory, 跑
         `frank memory takeover enable` 让 frank 接管" (一行命令但用户主动确认)
       - frank 自己不动配置文件
       - 真正接管推迟到 v0.10.7 (额外 4-7d 做 format-preserving)
    → POSITION.md 独家定位 3 "MCP 透明接管"暂留半截
```

## 风险 + 缓解(agent 5 条)

| ID | 风险 | Sev | 缓解 |
|---|---|---|---|
| **R1** | `.claude.json` 200K 行 re-serialize 破坏 history[] | **🔴 CRIT** | format-preserving byte-span splice,fixture invariant test 先行,失败降级文档方案 |
| R2 | opencode JSONC 注释丢失 | 🟡 MED | 优先字段切换不删 entry,最坏 warn"已规范化" |
| R3 | 接管中 backup 丢失 | 🟡 MED | restore fallback 给 GitHub 模板链接;state.json 存 entry hash sanity check |
| R4 | Headless / SSH 无 TTY 跑 install | 🟡 MED | TTY 检测,无 TTY 默认跳过 + log 提示;`FRANK_AUTO_TAKEOVER=1` env opt-in |
| R5 | Claude Code app 正在跑时改 ~/.claude.json | 🟡 MED | fsync + 提示用户重启 app 让新配置生效;不阻塞 |

## C 阶段 ship gate

- [ ] cargo test/clippy/fmt 全过
- [ ] **D3 fixture invariant test 必过**(200K .claude.json round-trip diff == entry 区块)
- [ ] 端到端真测 4 provider:`frank memory takeover enable` → official mcp_memory 在 4 provider 都 disable
- [ ] `frank memory takeover disable` 后 4 provider 恢复 official mcp_memory
- [ ] `frank memory takeover status` 显示当前 active_takeovers
- [ ] 端到端:`frank install <skill>` → 自动 takeover(TTY 模式 + 用户 confirm)
- [ ] 端到端:`frank cleanup` / `frank uninstall` → 自动 restore

## A 阶段 ship gate

- [ ] bump 0.10.5 → 0.10.6
- [ ] release.yml 6 平台全过
- [ ] Formula bump + caveats 加 "frank memory takeover enable" 一行
- [ ] brew upgrade + 真测 takeover lifecycle
- [ ] (无 sync-agent 改动,跳过 tx)
- [ ] 写 PHASE-6-LESSONS.md

## 不在 v0.10.6 范围

- 多设备同步状态(单机 takeover only,v0.12 多机)
- frank-mem MCP server 自家实现(那是 v0.12 Phase 12 的 #89)
- 用户手动改 config 期间的"冲突自动 merge"(只 warn,不自动 merge)

## 决策记录

- 采纳 agent GO-WITH-CAVEATS + D3 降级方案
- 4 provider 中 2 个走原生 `enabled=false`(简单),2 个走 entry-remove + backup(难)
- Homebrew Formula `def install` **不**自动跑 takeover(brew 装时无 sync-agent token,会 noop);走 caveats 提示用户主动跑
- 新命令 `frank memory takeover {status,enable,disable}`(不开新顶层命令,挂 memory 子树)
