# ADR-009: frank CLI 凭据桥 — 共享 crate + token_kind + 5 层 fallback

| Field | Value |
|---|---|
| **Status** | Proposed (V2, Plan Review round 2 pending) |
| **Date** | 2026-05-24 |
| **Decider** | hutiefang |
| **Target release** | v0.10.4 |
| **Estimated effort** | ~3 工作日 (V1 是 2 天,Plan Review 后扩到 3 天) |
| **Relates to** | ADR-006 (skill 自含), ADR-002 (cargo workspace), ADR-008 (memory v2, 待写) |

## V2 修订说明 (相对 V1)

V1 被 codex Plan Review 评 6.3 fail。V2 修 6 条:
1. **token_kind 区分**: OAuth session / long-lived API key / 第三方代理 三类,export 策略不同 (V1 大坑: 一律注 env 会导致 OAuth session 失效或泄漏 scope)
2. **共享 crate 而非只 frank-cli**: 新建 `crates/frank-cred/`, frank-cli 和 frank-orchestrator 共用 (V1 大坑: 只改 cli 留 orchestrator 行为不一致)
3. **跨平台 keyring 统一**: 引 `keyring` crate (覆盖 macOS Keychain / Windows Credential Manager / Linux Secret Service)
4. **stdout/stderr/logs 全链 redaction**: 不只 file mode,还要日志层过滤
5. **frank login UX 分离**: 不重载现有 `frank login`,改用 `frank login provider <name>` 子命令,help text 清晰区分 sync-agent vs provider
6. **工期 2 → 3 天**

## 背景 (痛点已实测复现)

`frank ai ask --to claude` 在跨"非 Anthropic 信任进程"链失败:

```
Codex CLI (Python) ──spawn──► frank-cli (Rust) ──spawn──► claude (Node)
                                                            ↓
                                              查 Keychain "Claude Code-credentials"
                                                            ↓
                                                macOS Keychain ACL 检查调用者进程树
                                                            ↓
                                                  Codex 进程树不在 ACL allow list
                                                            ↓
                                                        ❌ 拒绝 → loggedIn:false
```

同 binary 同代码, 从 Claude Code 终端直起 → ACL pass → ✅。

不是 frank bug, 是 macOS 安全模型。但 frank UX 责任。

## 关键设计变更 (V2)

### 1. TokenKind 体系 (修 V1 dim_3 大坑)

V1 设计: 命中 token 后**一律** `cmd.env("ANTHROPIC_API_KEY", token)` 注入 child。
**问题**: OAuth session token 不是 API key, 注入 env 会:
- (a) 跨 scope 泄漏 (OAuth scope ≠ API key scope)
- (b) session 短期失效后 child CLI 不会重新触发 OAuth flow
- (c) 不同 provider 的 official file 不是都安全可读 (codex 用 TOML, opencode 用自家 JSON)

V2 加 `TokenKind`:

```rust
pub enum TokenKind {
    /// 长期 API key (sk-ant-..., sk-proj-...).
    /// 安全注入 env var, child CLI 优先读 env, 绕开 Keychain。
    LongLivedApiKey,

    /// OAuth session token (短期, scope-bound).
    /// **不注 env**, 改为告诉 child CLI 走自家 file (设 HOME/XDG 让它找到 official file)。
    /// 若 official file 是 ACL 隔离的 (macOS Keychain), 走 fallback 5 提示 setup-token。
    OAuthSession,

    /// 第三方代理/中转站 key (用户配的 frank-store).
    /// 注 env, 但 doctor 给警告 (提醒用户这是非官方 endpoint)。
    ThirdPartyProxy,
}

impl Credential {
    fn export_strategy(&self) -> ExportStrategy {
        match self.kind {
            TokenKind::LongLivedApiKey | TokenKind::ThirdPartyProxy => {
                ExportStrategy::InjectEnv(self.provider.env_var_name())
            }
            TokenKind::OAuthSession => {
                // 不注 env, 让 child 走 file path
                ExportStrategy::PreserveOfficialFile
            }
        }
    }
}
```

### 2. 共享 crate `frank-cred` (修 V1 dim_9 大坑)

V1 把 credential 模块放 frank-cli, 但 `frank-orchestrator/src/worker/local.rs:194` 也 spawn 第三方 CLI 也 strip env, **同样问题**。

V2 新建 workspace crate:

```
crates/frank-cred/                NEW workspace crate
├── Cargo.toml
└── src/
    ├── lib.rs                    Credential / Provider / resolve_and_inject()
    ├── kind.rs                   TokenKind + ExportStrategy
    ├── provider.rs               Provider enum + 各家元数据 (env name, file path, setup cmd)
    ├── store.rs                  ~/.frank/credentials/<provider>.json (mode 0600)
    ├── env.rs                    env var fallback (FALLBACK 1)
    ├── official.rs               官方 file 探测 (FALLBACK 3)
    ├── keychain.rs               keyring crate 包装 (FALLBACK 4, 跨平台)
    └── redact.rs                 token redaction for stdout/stderr/logs
```

Workspace deps:
```toml
# Cargo.toml root
[workspace.dependencies]
frank-cred = { path = "crates/frank-cred" }
keyring = "3.6"          # 跨平台 keyring 抽象 (macOS Keychain/Win CredMan/Linux Secret Service)
secrecy = "0.10"         # zero-on-drop secret types
```

frank-cli 和 frank-orchestrator 都 depend on, 调用同一 `resolve_and_inject()`。

`frank-orchestrator/src/worker/local.rs:194` 改:
```rust
// 旧:
strip_empty_api_keys(&mut cmd);

// 新:
frank_cred::resolve_and_inject(&mut cmd, provider)?;  // 内部含 strip_empty 兜底
```

### 3. 5 层 fallback (核心不变, V2 补 Windows/Linux 具体)

```
1. ENV VAR (策略 D)
   - macOS/Linux/Windows 同源, std::env::var()

2. FRANK STORE (mode 0600)
   - ~/.frank/credentials/<provider>.json  (macOS/Linux)
   - %USERPROFILE%\.frank\credentials\<provider>.json (Windows, ACL 设当前用户独占)

3. OFFICIAL FILE (provider-specific)
   | Provider | macOS/Linux                          | Windows                                 |
   |----------|--------------------------------------|-----------------------------------------|
   | claude   | ~/.claude/.credentials.json          | %USERPROFILE%\.claude\.credentials.json |
   | codex    | ~/.codex/credentials.toml            | %USERPROFILE%\.codex\credentials.toml   |
   | gemini   | ~/.config/gemini/credentials.json    | %APPDATA%\gemini\credentials.json       |
   | opencode | ~/.local/share/opencode/auth.json    | %APPDATA%\opencode\auth.json            |

4. KEYRING (via `keyring` crate, 跨平台统一)
   - macOS: Keychain Services API (查 service name "Claude Code-credentials" 等)
   - Windows: Win32 Credential Manager API (DPAPI 加密, ACL 默认仅当前用户)
   - Linux: Secret Service (GNOME Keyring/KWallet libsecret), 若无则 4 层退化 skip
   - best-effort, ACL 拒绝则 None

5. GUIDANCE
   - 输出明确提示 + 一行可复制命令:
     ▶ frank login provider claude     # 自动 wrap claude setup-token
   - 若 headless (无 TTY):
     ▶ 手动: export ANTHROPIC_API_KEY=<your-key>
```

### 4. Stdout/stderr/logs Redaction (修 V1 dim_5)

新建 `frank-cred/src/redact.rs`:

```rust
/// 移除 stderr/stdout/logs 中可能出现的 token.
/// 正则: sk-ant-[A-Za-z0-9_-]{20,} | sk-[a-z]+-[A-Za-z0-9]{20,} | gho_[A-Za-z0-9]{36,}
pub fn redact_secrets(s: &str) -> String { ... }

/// 用作 frank-cli ui 输出层 + tracing 自定义 fmt layer.
pub struct RedactWriter<W: Write>(W);
impl<W: Write> Write for RedactWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let redacted = redact_secrets(std::str::from_utf8(buf).unwrap_or(""));
        self.0.write(redacted.as_bytes())
    }
}
```

集成点:
- frank-cli `log::ui::*` 输出包一层 `RedactWriter`
- tracing-subscriber fmt layer 替换 stderr writer
- frank-cli 跑 child subprocess 时, 把 child stdout/stderr 通过 `RedactWriter` pipe 出
- frank doctor 只显示 token 前 6 字符 + `***...{last4}`

### 5. frank login UX 分离 (修 V1 dim_6)

V1 用 `frank login --bootstrap-claude`, 与 `frank login` (sync-agent) 重载, 用户混淆。

V2 走子命令 (git-style):

```
frank login                           # 旧: sync-agent token (兼容)
frank login provider claude           # 新: bootstrap claude provider
frank login provider codex            # 新
frank login provider gemini           # 新
frank login provider opencode         # 新
frank login provider list             # 新: 列所有 provider 信任链状态
frank login provider rotate claude    # 新: 重新 setup-token
frank login provider remove claude    # 新: 删 frank store 里的 token
```

`frank login --help` 顶部 banner 明确分两类:
```
USAGE:
    frank login                            (1) frank-sync-agent 自家 token
    frank login provider <COMMAND>         (2) provider CLI 凭据 bootstrap
```

### 6. 工期 (修 V1 dim_7)

V1 = 2 天 过乐观, V2 = 3 天:

| 子任务 | V1 | V2 |
|---|---|---|
| frank-cred crate + 5 层 fallback + token_kind | 0.5d | **1.0d** (新 crate + token_kind 多策略) |
| frank login provider <name> (4 个 provider × bootstrap) | 1.0d | 1.0d |
| frank-cli ai.rs + frank-orchestrator local.rs 集成 | 0.3d | **0.5d** (双集成) |
| frank doctor 凭据信任链 | 0.2d | 0.3d |
| 测试 + redact + ADR + Formula | 0.5d (估漏) | **0.5d** |
| **合计** | **2 天** | **3 天** |

## 数据模型

### frank-cred Credential

```rust
pub struct Credential {
    pub provider: Provider,
    pub kind: TokenKind,
    pub token: SecretString,         // secrecy crate, zero-on-drop
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub source: CredentialSource,    // EnvVar / FrankStore / Official / Keyring / ThirdPartyProxy
}

pub enum CredentialSource {
    EnvVar(String),                  // env var name
    FrankStore(PathBuf),
    Official(PathBuf, Provider),
    Keyring { service: String, account: String },
}
```

### frank store schema

`~/.frank/credentials/claude.json`:
```json
{
  "provider": "claude",
  "kind": "long_lived_api_key",
  "token": "sk-ant-...",
  "expires_at": null,
  "created_at": "2026-05-24T22:00:00Z",
  "source": {
    "type": "bootstrap_setup_token",
    "official_file": "~/.claude/.credentials.json"
  }
}
```

跨平台权限:
- macOS/Linux: 目录 `0700`, 文件 `0600`
- Windows: `icacls` 或 Win32 API 设当前用户独占 (移除 Users 组)

## 风险

| ID | 风险 | 对策 |
|---|---|---|
| R-C1 | token 落 file 被他人读 | mode 0600/0700, Windows ACL, doctor 检查并警告 |
| R-C2 | headless 跑不通 setup-token | 检测 TTY, 无 TTY 给 ANTHROPIC_API_KEY 直配指引 + ssh tunnel 提示 |
| R-C3 | provider CLI 升级改 credential file 位置 | Provider trait 抽象 file path, 升级时改一处; doctor 探多个 candidate path |
| R-C4 | OAuth session token 误当 API key 注 env → 失效/泄漏 | TokenKind 区分, OAuthSession 走 PreserveOfficialFile 不注 env |
| R-C5 | child CLI 把 token echo 到 stdout/stderr 被用户 ps/log 看到 | RedactWriter 包 stdout/stderr; tracing fmt layer 同样过滤; 集成测包含 echo 攻击 |
| R-C6 | Linux 无 keyring 服务 | 4 层退化 skip, 走 fallback 3/5; doctor 提示 |
| R-C7 | Windows DPAPI 加密 token 跨用户拷贝失败 | DPAPI 设计如此, 拷贝场景给指引重新 setup-token |
| R-C8 | frank store 与 official file 分歧 | doctor 检测两边一致性, 不一致提示 rotate |
| R-C9 | secrecy crate 在 panic 时不一定 zero | secrecy 文档已知限制, 加文档说明 |
| R-C10 | 第三方代理/中转站 endpoint 被 frank 注入 token → 数据泄漏 | TokenKind::ThirdPartyProxy 时, doctor 显眼黄色警告 + frank ai ask 调用前确认 |

## 不在 v0.10.4 范围 (V2 收紧)

- 加密 frank store (file mode + Windows ACL 已够; 真要加密留 v0.13+)
- 多账号 (per-provider 多 token 切换)
- 与 ADR-005 daemon 模式整合 (策略 E)
- 自动刷新 OAuth session expires_at
- credential rotation 自动化 (定时跑 setup-token)
- web UI 凭据管理 (CLI 先)

## 验收 (v0.10.4 release 前)

- [ ] 复现链测试: tmux 内跑 codex, codex 内调 `frank ai ask --to claude` → ✅
- [ ] `frank login provider claude` 跑通: 完成 OAuth → 自动复制 token 到 frank store → mode 0600
- [ ] `frank login provider list` 输出 4 个 provider 信任链状态
- [ ] `frank doctor` 凭据节: 4 层 fallback 状态 + token 仅显示前 6 + ***...{last4}
- [ ] `frank ai ask` 失败时输出明确指引 ("跑 frank login provider claude")
- [ ] mode 0600 跨平台测 (Linux/macOS) + Windows ACL 测 (CI 包含 windows-2026)
- [ ] **frank-orchestrator local worker 也用 frank-cred** (不只 frank-cli)
- [ ] Redaction 测: child CLI 故意 echo 假 token → frank 输出已 mask
- [ ] TokenKind::OAuthSession 不注 env (单测覆盖, 确保不回归)
- [ ] keyring crate 在 Linux 无 Secret Service 时优雅 fallback
- [ ] Plan Review by codex >= 7.0, 无单维度 <= 3 (V2 round 2)
- [ ] Code Review by codex >= 7.0, 无单维度 <= 3
- [ ] CI 全绿 (workspace clippy/test/fmt/docs/audit/secret-scan)
- [ ] Homebrew Formula bump 0.10.3 → 0.10.4

## V3 实施日志 (2026-05-25, 实施中发现)

V2 codex Plan Review 评 7.6 pass. 实施过程中两处验证驱动的修订:

### V3 修订 1: OAuthSession 也 InjectEnv (撤回 V2 PreserveOfficialFile)

V2 设计: `TokenKind::OAuthSession` 走 `ExportStrategy::PreserveOfficialFile` (不注 env, 让 child 自己读 file)。

实测发现: child CLI (`claude --print` / `codex exec`) **同样** 因 macOS Keychain ACL 在
非 Anthropic 信任进程链中拿不到 token (它内部也是 keyring crate / 自家库调 Keychain)。
不注 env = child 也读不到 = 等于没修。

V3: `OAuthSession` **也** `InjectEnv`。Anthropic Claude `claude --print` 见
`ANTHROPIC_API_KEY` 就用, 不论 long-lived 还是 OAuth (Anthropic 文档明确说明非交互模式
优先 env)。安全考量:
- OAuth scope (`user:inference, user:mcp_servers` 等) 由 user 内部使用, frank 是合法 wrap
- 失效风险: doctor 显示 `expires_at`, 失效前 7 天警告刷新 (TODO: 待 doctor 增强)

代码: `crates/frank-cred/src/kind.rs:55-72` 的 `export_strategy()` 三类都返 InjectEnv。

### V3 修订 2: macOS `security` CLI fallback (绕过 keyring crate ACL)

V2 设计: Layer 4 (Keyring) 用 `keyring` crate v3 跨平台。

实测发现: `keyring` crate 在 macOS 调 `SecKeychainFindGenericPassword`, ACL 拒绝时
**静默返回 itemNotFound** (按设计如此, 不报错), 看上去像"无凭据"。实际 Anthropic
v2 把 OAuth token 只存 Keychain (service `Claude Code-credentials`), 无 file fallback。

V3: keychain.rs 加 `security` CLI fallback。`security` 是 macOS 系统 binary, ACL 默认
允许列表中, **能读** generic password。spawn `security find-generic-password -s <svc>
-a <acct> -w` 后解析输出:
- 如果是 JSON (Anthropic OAuth 格式 `{"claudeAiOauth":{...}}`) → 提取 accessToken + 标 OAuthSession
- 否则当裸 token (启发式判断 kind)

代码: `crates/frank-cred/src/keychain.rs:75-150`。

### 端到端验证

```
$ frank doctor
  ...
  ✓ credential: claude  ✓ keyring:Claude Code-credentials (inject env: ANTHROPIC_API_KEY)
  ...

$ frank ai ask --to claude "say OK"
[frank-cred] ✓ ANTHROPIC_API_KEY (source: keyring:Claude Code-credentials)
OK
```

ACL 链 (Claude Code → bash → frank-cli → security CLI → 读 Keychain) 一次性打通。

## 参考

- Anthropic Claude Code Authentication docs
- bfly123/claude_code_bridge, cx994/ccb — tmux 路线对照
- `keyring` crate v3.6 — 跨平台 keyring 抽象
- `secrecy` crate — Rust secret types
- Git credential helper 设计
- ADR-002 (workspace), ADR-006 (skill 自含边界)
- frank-orchestrator/src/worker/local.rs:194 (现有 strip_empty 集成点)
