# PHASE-10 计划: v0.12.0 — Server Tenant Registry + 防滥用 + 删除流程 + 安装即体验

> 用户 2026-05-25 拍板范围 (D 选项 "全干完一次 ship")
> 工程量预估: 3-5 工作日 (server 改造 + 客户端流程 + Formula post_install)

---

## 0. 决策回顾 (用户反馈,2026-05-25)

| 我之前提的 | 用户驳回 | 新设计 |
|---|---|---|
| 任意非空 token = tenant | 攻击者可发送几亿条 spam | **必须 server 端注册** + **配额** |
| 14 天自动 retention | 应该用户**申请删除**才有 retention | 默认永久存; `frank tenant delete` 申请 + 14 天后真删 |
| 5 个端点 | — | 8 个 + 自动 token + brew post_install + CLAUDE.md 注入 + 自动摘要 |

---

## 1. 范围 (8 个子项 A-H)

| # | 子项 | 端 | 工期 | 优先 |
|---|---|---|---|---|
| A | Server tenant registry (SQLite) | server | 1 d | P0 |
| B | Quota 配额 (默认 10k records) | server | 0.5 d | P0 |
| C | frank 首次自动 token + 注册 | client | 0.5 d | P0 |
| D | 删除流程 CLI + UX 倒计时 | client | 0.5 d | P0 |
| E | retention worker 真删除 | server | 0.5 d | P0 |
| F | brew install 自动 frank hook install | install | 0.5 d | P1 |
| G | CLAUDE.md 自动注入 frank-memory 介绍 | client | 0.5 d | P1 |
| H | frank ai ask 后自动摘要落记忆 | client | 1 d | P1 |

---

## 2. Server 端设计 (A+B+E)

### 2.1 数据 schema (SQLite `~/.frank/tenants.db` on tx)

```sql
CREATE TABLE tenants (
  tenant_id TEXT PRIMARY KEY,        -- sha256(token) 前 12 hex 字符
  created_at INTEGER NOT NULL,       -- unix epoch sec, 注册时间
  last_seen INTEGER NOT NULL,        -- 最后访问 (任何操作更新)
  records_count INTEGER NOT NULL DEFAULT 0,  -- 已用 quota
  deletion_scheduled_at INTEGER       -- NULL = 不删除; >0 = 14 天后真删 epoch
);
CREATE INDEX idx_deletion ON tenants(deletion_scheduled_at)
  WHERE deletion_scheduled_at IS NOT NULL;
```

### 2.2 端点设计

| Method | Path | 作用 | 鉴权 |
|---|---|---|---|
| POST | `/tenant/register` | 注册新 tenant (idempotent) | X-Frank-Token (any) |
| GET | `/tenant/status` | 看自己: quota 用量 / 删除状态倒计时 | X-Frank-Token (must registered) |
| POST | `/tenant/request-deletion` | 申请删除, server schedule 14d | X-Frank-Token (must registered) |
| POST | `/tenant/cancel-deletion` | 取消申请 | X-Frank-Token (must registered) |
| POST | `/memory/add` | (现有, 加 quota check + tenant must registered) | 同 |
| POST | `/memory/add_raw` | (现有, 同上) | 同 |
| POST | `/memory/search` | (现有, 必须 registered) | 同 |
| POST | `/memory/list` | (现有, 必须 registered) | 同 |
| DELETE | `/memory/:id` | (现有, 必须 registered, quota -=1) | 同 |

### 2.3 防滥用

1. **未注册 token 写入直接 401** (现在是任意 token 都接受)
2. **`/tenant/register` 限频**: 同 IP 1 个/15 min (Caddy `rate_limit` plugin 或 sync-agent 内部 token bucket)
3. **配额上限**: 默认 10000 records (server `FRANK_QUOTA_PER_TENANT` env 可调)
4. **超额拒绝**: HTTP 429 with body `{"error": "quota_exceeded", "limit": 10000, "used": 10000}`

### 2.4 retention worker (E)

`tokio::spawn` 后台任务,每小时扫一次:

```rust
async fn retention_worker(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let now = chrono::Utc::now().timestamp();
        let due: Vec<TenantId> = state.db.query("SELECT tenant_id FROM tenants WHERE deletion_scheduled_at <= ?", [now])?;
        for tenant_id in due {
            // 1. qdrant delete_points filter by user_id = "t_<tenant>"
            state.qdrant.delete_by_filter(&format!("t_{tenant_id}")).await?;
            // 2. sqlite delete row
            state.db.exec("DELETE FROM tenants WHERE tenant_id = ?", [&tenant_id])?;
            tracing::info!(tenant_id, "real-deleted (retention 14d expired)");
        }
    }
}
```

**真删除 — qdrant `delete_points` + sqlite `DELETE`, 不是软删除 (用户原话)。**

---

## 3. Client 端设计 (C+D+F+G+H)

### 3.1 C — 首次自动 token

`frank` 任何命令启动时,`sync_client::from_env_or_config()` 内部加:

```rust
if token_path.exists() == false {
    let token = uuid::Uuid::new_v4().to_string();
    std::fs::write(&token_path, &token)?;
    std::fs::set_permissions(&token_path, 0o600)?;  // chmod 600
    // 调 /tenant/register
    let client = reqwest::blocking::Client::new();
    client.post(format!("{base_url}/tenant/register"))
        .header("X-Frank-Token", &token)
        .send()?;
    crate::log::ui::success("frank 首次启动: 已生成随机 token 并注册到服务器");
}
```

失败时 (网络 / 限频) 给清晰提示,但不阻塞本地命令 (用户可以稍后手动 `frank login`)。

### 3.2 D — 删除流程

3 个新 CLI 子命令:
- `frank tenant delete` — POST /tenant/request-deletion, server 设 `deletion_scheduled_at = now + 14d`,客户端显示 "你的数据将在 2026-06-08 14:30 真删,期间可跑 `frank tenant cancel-delete` 撤销"
- `frank tenant cancel-delete` — POST /tenant/cancel-deletion
- `frank tenant status` — GET /tenant/status, 表格显示 `tenant_id / created_at / quota / deletion_status`

**惰性日提醒**: 在 `frank memory list/search/add` 调 sync-agent 时,如果服务端返回的 status 含 `deletion_scheduled_at`,客户端 cache 24 小时,每天提醒一次:

```
⏰ 你申请的数据将在 X 天后 (2026-06-08 14:30) 真删, 取消: frank tenant cancel-delete
```

### 3.3 F — Homebrew post_install 自动装 hook

`Formula/frank.rb`:

```ruby
def post_install
  # 只在用户有 Claude Code 时装 (~/.claude/settings.json 存在)
  settings = "#{ENV["HOME"]}/.claude/settings.json"
  if File.exist?(settings)
    system "#{bin}/frank", "hook", "install"
  end
end
```

### 3.4 G — CLAUDE.md 注入

`frank hook install` 同时 append 到 `~/.claude/CLAUDE.md` (无则建):

```markdown
<!-- BEGIN frank-memory (managed by frank hook install) -->
## frank-memory

你有访问用户分布式记忆的能力:
- 想找之前的事: 跑 `frank memory search "<query>" --user <username>`
- 想存新事: 跑 `frank memory add "<内容>" --user <username>`
- 列表: `frank memory list --user <username>`

数据存到 frank.hutiefang.com (用户独立 tenant, sha256 隔离).
<!-- END frank-memory -->
```

**幂等**: 检测 BEGIN/END 标记,已存在跳过。`frank hook uninstall` 反向清掉。

### 3.5 H — 自动摘要

`frank ai ask` 拿到 AI 回答后,**异步** spawn 一个后台任务:

```rust
tokio::spawn(async move {
    let summary = extract_facts_via_cli("claude", &format!(
        "对话摘要:\nUSER: {prompt}\nAI: {response}\n\n抽出 1-3 条事实存为长期记忆"
    )).await?;
    for fact in summary {
        sync_client.add_raw(&fact, &scope, None)?;
    }
});
```

默认开,`--no-auto-save` 关。不阻塞用户主流程。

---

## 4. 边界 (不做的事)

按用户范围确认,**不在 v0.12.0**:
- ❌ 多 token 合并 (`frank tenant merge`) — 用户之前列了但 D 接受,推 v0.12.1
- ❌ 邮件通知 (申请删除 → 发邮件) — 太重,SMTP 配置麻烦, 留 v0.13
- ❌ 7-day reminder before deletion — 推 v0.12.1
- ❌ Web UI 删除入口 — 推 v0.12.1

---

## 5. 验收

| 维度 | 验收门 |
|---|---|
| 服务端 SQLite | `~/.frank/tenants.db` 真生成, 端点 4 个真通 |
| 配额拒绝 | 写 10001 条 → 第 10001 条 429 quota_exceeded |
| 首次注册 | 全新机器 brew install + 跑任意 frank 命令 → token 自动生成 + 注册成功 |
| 删除倒计时 | `frank tenant delete` → status 显示 14d 倒计时 → cancel → 倒计时消失 |
| 真删除 | 改 `deletion_scheduled_at` 到 1 秒前 → 等 1 hour worker tick → qdrant points 真消失 |
| hook 自动装 | brew install → `frank hook status` 显示已注册 |
| CLAUDE.md 注入 | `cat ~/.claude/CLAUDE.md \| grep frank-memory` 找到 |
| 自动摘要 | `frank ai ask "我喜欢 vim"` 后 `frank memory list` 看到相关 fact |

---

*最后更新: 2026-05-25. Status: Proposed, 用户拍板 D 后开干.*
