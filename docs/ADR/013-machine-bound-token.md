# ADR-013: 机器绑定 token + 服务端控制生成 (v0.13.0)

| Field | Value |
|---|---|
| **Status** | Proposed (待 codex Plan Review + 用户拍板) |
| **Date** | 2026-05-25 |
| **Decider** | hutiefang |
| **Depends on** | ADR-005 (tx:8318 部署), PHASE-10-PLAN (v0.12.0 tenant registry) |
| **Target release** | v0.13.0 (PHASE-11 子项 A — 安全收口) |
| **POSITION 引用** | 维度 #11 device token 解耦, 撤回项 ❌ A (服务端 spam 风险) |
| **Estimated effort** | 2-3 工作日 (Agent A 客户端 1d + Agent B 服务端 1d + E2E 0.5d) |

## 1. 背景

v0.12.0 落地了 server tenant registry (SQLite + quota 10k records/tenant + 14d retention), 解决了**单 tenant 内**的存储滥用, 但**未解决** tenant 行本身的无限注册。

用户 2026-05-25 复查时点出的威胁:

> "10k 条 quota 是按 tenant 算的, 但 token 我自己生成。客户端跑 for loop 起 100w 个 uuid 各写 1 条, quota 永远不超, tenants 表照样炸。"

复现 (30 行 Python + 1 台云服务器):

```python
for _ in range(10_000_000):
    t = str(uuid.uuid4())
    requests.post(f"{URL}/tenant/register", headers={"X-Frank-Token": t})
    requests.post(f"{URL}/memory/add_raw", headers={"X-Frank-Token": t},
                  json={"content": "x", "scope": {"user_id": "u"}})
```

实测 30s 涨 ~3000 tenant 行 / ~3000 qdrant points。按这速度 24h 可塞 ~8.6M 行, 撑爆 tx VM disk。Caddy IP rate_limit 1 req/15min/IP 在多 IP / VPS 攻击下几乎无效 (一个 /24 段 256 IP)。

**核心**: token 客户端生成 = 服务端**无法**限制创建速率。

## 2. 决策

**v0.13.0 把 token 生成权从客户端收回到服务端**, 客户端先发**机器指纹**, 服务端校验**指纹未注册过**才返回 token。

设计简称: **machine-bound token** (一台物理机 1 个 tenant, 默认 1:1)。跨机靠**显式** `frank tenant link` 命令把同账号多机加进同一 tenant (1:N, 用户主动)。

参考案例:
- **Stripe API keys** — 全在 dashboard server 生成 (32 字节 random + key prefix), 客户端不可拼。
- **1Password device fingerprint** — 设备激活时上传 fingerprint, 服务端绑 account_id, 同 fingerprint 再激活直接 reuse, 不发新 device_id。
- **RFC 4122 §4.4** (uuid v4) 只标 122 bit 随机性, 不够对抗专门 attacker; **NIST SP 800-90A** 推荐 ≥ 128 bit, 取 **256 bit** (32 字节 getrandom)。

## 3. 数据流详图

### 3.1 新机器首次跑 `frank <任意命令>`

```
client (~/.frank/.token 不存在)
  │
  │ 1. fp = machine_id::collect_fingerprint()
  │ 2. POST /tenant/provision  body: { fingerprint: fp }
  ▼
server
  │ 3. machine_code = sha256(canonical_json(fp))[..16hex]
  │ 4. SELECT 1 FROM machines WHERE machine_code = ?
  │     ├── 命中: 409 {"error": "machine_already_registered", "hint": "<提示 link/reset 命令>"}
  │     └── miss: 继续
  │ 5. token = base64url_no_pad(getrandom(32))     // 256 bit
  │    tenant_id = sha256(token)[..12hex]
  │ 6. BEGIN; INSERT tenants ...; INSERT machines ...; COMMIT
  │ 7. 200 { token, tenant_id, machine_code }
  ▼
client
  │ 8. write ~/.frank/.token (mode 0600)
  │    write ~/.frank/.machine_id (mode 0644, info-only)
  │ 9. ui::success("frank: 已注册新机器 ({machine_code[..8]})")
```

### 3.2 同机器后续调用

走 v0.12.0 老路径, 不变: `client 读 .token → header X-Frank-Token → server sha256 → tenant_id`。machine_code 不参与运行时鉴权 (只是注册时的防重种子), 每次跑 frank 不重算 fingerprint, 0 性能开销。

### 3.3 加新机器 — `frank tenant link --token <existing>`

用户在 linux 上有 token T, 想在 mac 看同一份记忆:

```
1. user: frank tenant link --token <T>   (T 用户从 linux 上 cat ~/.frank/.token 拿到)
2. client: fp = collect_fingerprint()
3. POST /tenant/link-machine  header: X-Frank-Token: <T>  body: { fingerprint }
4. server: tenant_id = sha256(T)[..12hex]; 校验 tenant 存在 (否则 401)
5. server: machine_code = sha256(canonical_json(fp))[..16hex]
6. SELECT tenant_id FROM machines WHERE machine_code = ?
     ├── 命中 ≠ T 的 tenant_id: 409 (这台机器已属别人)
     ├── 命中 == T 的 tenant_id: 200 (幂等, 重复 link 不报错)
     └── miss: INSERT machines, 200
7. client: write ~/.frank/.token (= T) + ~/.frank/.machine_id
```

### 3.4 重置 — `frank tenant reset --confirm`

```
1. user: frank tenant reset --confirm
2. client: 读旧 .token + 算 fp
3. POST /tenant/unlink-machine { fingerprint }
   server: DELETE FROM machines WHERE machine_code=? AND tenant_id=?
4. client: rm ~/.frank/.token, rm ~/.frank/.machine_id
5. 下次跑 frank → 触发 §3.1 新 provision (新 token + 新 tenant)
```

注意 `reset` **不删 tenants 行也不删 Qdrant points** — 用户可能后悔, 14d retention 兜底。要彻底删数据走 `frank tenant delete` (v0.12.0 已有)。

## 4. Schema 变更

`tenants` 表 (v0.12.0) **不动**。**新增** `machines` 表:

```sql
CREATE TABLE machines (
  machine_code      TEXT PRIMARY KEY,        -- sha256(canonical_json(fingerprint))[..16hex]
  tenant_id         TEXT NOT NULL,
  fingerprint_json  TEXT NOT NULL,           -- 调试用, 不参与查询
  created_at        INTEGER NOT NULL,
  last_seen         INTEGER NOT NULL,
  FOREIGN KEY (tenant_id) REFERENCES tenants(tenant_id) ON DELETE CASCADE
);
CREATE INDEX idx_machines_tenant ON machines(tenant_id);
```

`ON DELETE CASCADE`: `frank tenant delete` 真删 tenant 时 machines 行随删, 一致性保证。

## 5. Fingerprint 字段

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Fingerprint {
    pub hostname: String,            // gethostname
    pub mac_addresses: Vec<String>,  // 物理网卡 MAC, 排序去重 (Vec 不是 HashSet, 顺序确定)
    pub os: String,                  // "linux" / "macos" / "windows"
    pub os_version: String,          // 只取 major, 如 "15" 不取 "15.3.1" (避免点更新触发误判)
    pub cpu_brand: String,           // sysinfo
    pub cpu_cores: u32,              // num_cpus::get_physical()
    pub total_memory_mb: u64,
}
```

序列化**必须**用 struct 顺序 (不要 HashMap → 顺序不定 → sha256 不一致)。

**故意不取** (隐私 + 不稳定):
- IP 地址 — 换 WiFi 就变, 暴露用户位置
- 硬盘序列号 / BIOS UUID — 跨 OS 不一致, 需 root
- 用户名 — 用户自己改

**接受的不稳定**: MAC 换网卡 / hostname 手改 → 触发 provision 新 tenant → 用户跑 `frank tenant link` 找回。

## 6. Token 生成

```rust
// frank-sync-agent/src/auth.rs
pub fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);   // 底层 getrandom syscall
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)  // ~43 字符
}
```

| 维度 | uuid v4 (v0.12.0) | v0.13.0 |
|---|---|---|
| 熵 | 122 bit | **256 bit** |
| 字符集 | hex + `-` (36 chars) | base64url no_pad (43 chars) |
| 服务端控制 | ❌ 客户端自生成 | ✅ server 生成 |

服务端生成 = 客户端**无法**枚举有效 token (2^256 暴力穷举物理不可行)。

## 7. 威胁模型

### 防什么 (v0.13.0 解决)

| 威胁 | v0.12.0 中招? | v0.13.0 缓解 |
|---|---|---|
| 单一物理机器, 多线程注册百万 token (用户原话) | ✅ | machine_code 第 2 次 409 |
| uuid v4 暴力穷举碰撞既有 tenant | 122 bit 理论可能 | 256 bit 物理不可行 |
| token 泄漏后他人无限注册新 tenant 占额 | ✅ | 别人也要过指纹关 |

### 不防什么 (留 v0.14)

| 威胁 | 为何不防 |
|---|---|
| VM 集群 spam — 1000 docker container 各自 hostname/MAC 不同 | fingerprint 确实不同, server 看是 1000 台机器, 合法注册。**v0.14 加 IP rate limit + proof-of-work** |
| MAC spoof 同台机器改 MAC | 同 VM 效果 |
| 拷贝整个 `~/.frank/` 到另一台机器 | 服务端无从知道 (token 一样), 这是**用户预期**的多机使用 |

**结论**: v0.13.0 防"懒 spammer + 单机暴力", 不防"专业 attacker"。POSITION 撤回项里 frank 不做"全行业最完美安全", 挡住大多数 spam 即可。

### Friendly use 误差

| 场景 | 反应 | 应对 |
|---|---|---|
| 换网卡 / 重装系统 / macOS major 升级 | fp 变 → provision 新 tenant; 老 tenant 留服务器 | `frank tenant link --token <旧>` 找回; 或 `frank tenant delete` 14d 后删老的 |
| 同事用同一台云开发机的两个账号 | 同 fp = 同 tenant = 数据混 | 文档警告: 共享物理机不要用 frank-memory; user-scoped sub-tenant 留 v0.15+ |
| Docker 反复 build (每次 fp 不同) | 触发 spam tenant | 文档推荐 mount host 的 `~/.frank/.token` 进容器 |

## 8. 客户端 frank-cli 改动 (Agent A)

新 module `crates/frank-cli/src/machine_id.rs`:

```rust
pub struct Fingerprint { /* §5 字段 */ }
pub fn collect_fingerprint() -> anyhow::Result<Fingerprint> { ... }
pub fn canonical_json(fp: &Fingerprint) -> String {
    serde_json::to_string(fp).expect("infallible struct ser")
}
```

`sync_client.rs::auto_provision_token()` (v0.12.0 引入, `uuid::Uuid::new_v4`) 改成调 POST `/tenant/provision`, 不再本地 uuid:

```rust
fn auto_provision_token(base_url: &str) -> anyhow::Result<String> {
    let fp = machine_id::collect_fingerprint()?;
    let resp = blocking_client()
        .post(format!("{base_url}/tenant/provision"))
        .json(&json!({ "fingerprint": fp })).send()?;
    match resp.status().as_u16() {
        200 => { /* write token + machine_id */ Ok(token) }
        409 => bail!("此机器已注册过; 跑 `frank tenant link --token <existing>` \
                      或 `frank tenant reset --confirm`"),
        s => bail!("provision 失败 HTTP {}", s),
    }
}
```

依赖新增: `mac_address = "2.0"` (跨平台读 MAC, 无 root)。`hostname` / `sysinfo` workspace 已有。

## 9. 服务端 frank-sync-agent 改动 (Agent B)

### 9.1 schema migration

启动时检测 `tenants.db` 无 `machines` 表 → 创建。**不**做老数据迁移 (v0.12.0 的少量孤儿 tenant 留着, 不影响新数据)。

### 9.2 新增端点

```
POST /tenant/provision        body: { fingerprint }
  200: { token, tenant_id, machine_code }
  409: { error: "machine_already_registered", machine_code, hint }

POST /tenant/link-machine     header: X-Frank-Token: <existing>  body: { fingerprint }
  200: { tenant_id, machine_code }
  401: tenant_id 不存在
  409: machine 属于别的 tenant

POST /tenant/unlink-machine   header: X-Frank-Token  body: { fingerprint }
  200: {} (幂等, 不存在也成功)
```

### 9.3 老端点 `/tenant/register` 处理

**deprecated 但保留** 2 个版本 (v0.13/v0.14), 内部转调 `provision` 但记 warn 日志 (rolling upgrade 期间老客户端能跑)。v0.15 删掉。

## 10. 新 CLI 子命令

```
frank tenant link --token <T>      # §3.3
frank tenant reset --confirm       # §3.4 (--confirm 防误用)
frank tenant machines              # GET /tenant/machines, 列本 tenant 的全部 machine_code
                                   # + hostname + os + last_seen (从 fingerprint_json 取)
```

`frank tenant status` (v0.12.0 已有) 输出加一行 `machines: 2` 显示数量。

## 11. 后果

**优点**:
- 防 spam — 单机注册 1:1, 第 2 次必 409
- token entropy 122 bit → 256 bit (2^134 倍提升)
- 服务端**完全控制** token lifecycle, 后续可加 audit log / 强制 rotate
- machines 表自带 fingerprint_json, 未来加 device 列表 UI 几乎零工作

**缺点**:
- 换硬件流程繁琐 (要用户手动 `tenant link`) — 错误消息直接给命令模板缓解
- fingerprint spoofing 不防 (见 §7) — 留 v0.14
- SQLite 大小涨 — machines 表每行 ~500 字节, 10w 机器 ≈ 50 MB, 可接受
- 新增 `mac_address` 依赖, Linux 上要读 `/sys/class/net/*/address`, CI 实测验证

**应对**:

| 风险 | 应对 |
|---|---|
| 用户换网卡懵 | 409 错误消息**直接给** `frank tenant link --token <旧>` 命令; doctor 提示 |
| fingerprint 跨 OS 不一致 | os_version 只取 major ("15" 不取 "15.3.1") |
| 隐私担忧 | DESIGN.md 加节披露上传字段, 明示不传 IP / 序列号 |
| machines 无限增长 | retention worker 扫 `last_seen < now - 365d` 自动清, 单独 PR (不在本 ADR) |

## 12. 不在 v0.13.0 范围 (推 v0.14+)

- IP rate limit 在 `/tenant/provision` (5 req/IP/day) — 见 §7
- Proof-of-work challenge — provision 前要算 `SHA-256(N || nonce) < threshold`, 增加 spam 成本
- Web UI 显示 machine 列表
- 强制 rotate (90d 重发 token)
- 跨用户 user-scoped sub-tenant (共享物理机场景)
- machines 表自动清理 worker (单独 PR)

## 13. 验收

### 单测

**Agent A** (frank-cli, ≥ 6 用例):
- [ ] `collect_fingerprint_macos` — macOS 真机, 字段全非空
- [ ] `collect_fingerprint_linux` — Linux CI runner
- [ ] `canonical_json_deterministic` — 同 Fingerprint 序列化 3 次完全相等
- [ ] `canonical_json_field_order_regression` — 打乱源码字段顺序后 sha256 应变 (回归测)
- [ ] `auto_provision_token_409` — mock 返 409 → 报错含 `frank tenant link` 提示
- [ ] `mac_address_sorted_dedup` — 多网卡 MAC 列表排序+去重

**Agent B** (frank-sync-agent, ≥ 4 用例):
- [ ] `provision_new_machine_ok` — fresh fp → 200 + token + machines 行
- [ ] `provision_duplicate_machine_409` — 同 fp 2 次 → 第 2 次 409
- [ ] `link_machine_ok` — 已有 tenant T 跑 link → machines 多一行 tenant_id=T
- [ ] `link_machine_conflict` — fp 已属别人 → 409

### 端到端

- [ ] 全新 ubuntu VM + 全空 `~/.frank/` → `frank memory list` → 自动 provision 成功 → `.token` 写盘 + `stat -c %a` = 600
- [ ] 同 VM 删 `.token` 再跑 → 第 2 次 provision 应 409 + 错误含 `frank tenant link` 模板
- [ ] **跨机**: Linux cat token T → mac 跑 `frank tenant link --token <T>` → mac `frank memory list` 看到 Linux records
- [ ] **重置**: mac 跑 `frank tenant reset --confirm` → 服务端 machines 删 mac 行 → 再跑触发新 provision (新 tenant)

### CI

- [ ] workspace clippy / test / fmt / docs 全绿
- [ ] secret-scan 不报警 (fingerprint_json 别意外混入测试 fixture)
- [ ] Plan Review by codex ≥ 7.0 且无维度 ≤ 3
- [ ] Code Review by codex ≥ 7.0 且无维度 ≤ 3

## 14. 参考

- `docs/POSITION.md` — 维度 #11 device token 解耦, 撤回项 ❌ A
- `docs/phases/PHASE-10-PLAN.md` — v0.12.0 tenant registry / quota / retention 基础
- `docs/ADR/005-deploy-tencent-8317.md` — tx:8318 服务端拓扑
- `docs/ADR/009-cli-credential-bridge.md` — 凭据存盘 mode 0600 复用同套
- Stripe API key design — https://stripe.com/docs/keys
- 1Password device authentication white paper §6 — https://1passwordstatic.com/files/security/1password-white-paper.pdf
- RFC 4122 §4.4 (uuid v4 / 122 bit) — https://datatracker.ietf.org/doc/html/rfc4122
- RFC 4648 §5 (base64url) — https://datatracker.ietf.org/doc/html/rfc4648
- NIST SP 800-90A — random bit generation 标准
- `mac_address` crate v2.0 — https://crates.io/crates/mac_address
- `sysinfo` crate — workspace 已有
