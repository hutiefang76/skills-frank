# Frank Official 大数据/Flink 周边 skill 缺口分析与补齐提议

> 文档目的:对 `crates/frank-cli/manifest/builtin.yaml` 现状做审计,识别 `frank-official` (项目作者 hutiefang76 自研) 系列在 "Flink + 大数据 + DevOps" 方向上的缺口,提议 12 个候选新增 skill。**只提议、不实现、不改 manifest**。
>
> 状态: Draft · 作者: agent audit · 日期: 2026-05-25 · 待用户 review。

---

## 1. 现状摘要 (14 个 skill 一览)

`builtin.yaml` 实际只有两类 visibility 被使用:`frank-recommended` (curated 第三方) 与 `frank-official` (项目方自研)。`schema_version: 1`,`profile: personal`。

| # | name | visibility | source kind | url / spec | description |
|---|---|---|---|---|---|
| 1  | frank-ask-gpt           | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-ask-gpt           | 转发给 codex CLI (gpt-5.5 Plus) |
| 2  | frank-ask-claude        | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-ask-claude        | 转发给 claude CLI (Pro/Max opus) |
| 3  | frank-ask-opencode      | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-ask-opencode      | 转发给 opencode CLI (qwen3.6+) |
| 4  | frank-ask-gemini        | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-ask-gemini        | 转发给 gemini CLI (Google) |
| 5  | frank-mem-list          | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-mem-list          | 列分布式记忆 |
| 6  | frank-mem-search        | frank-recommended | git (subpath)  | github.com/hutiefang76/skills-frank-bridge.git#frank-mem-search        | 语义检索记忆 |
| 7  | skill-creator           | frank-recommended | git (subpath)  | github.com/anthropics/skills.git#skills/skill-creator                  | 元 skill — 创建/迭代 skill |
| 8  | superpowers             | frank-recommended | git            | github.com/obra/superpowers.git                                        | 通用编程超能力 (TDD/debug) |
| 9  | mcp-time                | frank-recommended | mcp (npx)      | @modelcontextprotocol/server-time                                      | 时间/时区 MCP |
| 10 | mcp-sequential-thinking | frank-recommended | mcp (npx)      | @modelcontextprotocol/server-sequential-thinking                       | 分步推理 MCP |
| 11 | mcp-fetch               | frank-recommended | mcp (uvx)      | mcp-server-fetch                                                       | HTTP fetch MCP |
| 12 | mcp-context7            | frank-recommended | mcp (npx)      | @upstash/context7-mcp                                                  | 第三方库文档 MCP |
| 13 | **nacos-ops**           | **frank-official** | git           | github.com/hutiefang76/skills-nacos-ops.git (ref=master)               | Nacos 配置中心运维 |
| 14 | **streampark-ops**      | **frank-official** | git           | github.com/hutiefang76/skills-streampark-ops.git                       | StreamPark Flink 作业平台运维 |

注释里还有 3 个被预留但未启用的占位 (doris-ops / feishu-read / dolphinscheduler-ops),仓库 404。

### URL 健康度 (git ls-remote 探活)

| URL | 状态 |
|---|---|
| skills-frank-bridge.git                | OK |
| anthropics/skills.git                  | OK |
| obra/superpowers.git                   | OK |
| skills-nacos-ops.git                   | OK |
| skills-streampark-ops.git              | OK |
| skills-doris-ops.git                   | **404** (注释正确预告) |
| skills-feishu-read.git                 | **404** (注释正确预告) |
| skills-dolphinscheduler-ops.git        | **404** (注释正确预告) |

**结论**: 5 个真在用的 git URL 全部健康;3 个 404 在 yaml 中已被注释,只是设计意图占位,无误导。

---

## 2. 缺口分析 — 用户期望 vs 现状

用户原话:"frank-skills 管理了一堆 flink 自研的工具 skills 比如 nacos 等等"。"**一堆**"暗示 official 系列应在 8-15 个量级覆盖大数据 + Flink 栈,但现状只有 **2 个**:

- `nacos-ops` (配置中心)
- `streampark-ops` (Flink 平台)

**典型大数据栈分层** vs **现状**:

| 层 | 典型组件 | frank-official 现状 |
|---|---|---|
| 调度 / 编排  | DolphinScheduler / Airflow / Azkaban       | 占位 (404) |
| 流计算       | Flink 平台 / Job / Checkpoint              | streampark-ops (平台层); 缺 job-level 监控 |
| 批计算       | Spark / Tez                                 | **缺** |
| 消息          | Kafka / Pulsar / RocketMQ                  | **缺** |
| 列存 OLAP    | Doris / StarRocks / ClickHouse             | 占位 (404 doris) |
| 数据湖       | Iceberg / Hudi / Paimon                    | **缺** |
| 元数据       | Hive Metastore / DataHub                   | **缺** |
| 存储          | HDFS / OSS / MinIO                         | **缺** |
| KV / 缓存    | Redis / HBase                              | **缺** |
| 协调          | ZooKeeper / etcd                           | **缺** |
| 配置          | Nacos / Apollo / Consul                    | nacos-ops |
| 检索          | Elasticsearch / OpenSearch                 | **缺** |
| 文档抽取     | 飞书 / 钉钉 / 企微                          | 占位 (404 feishu-read) |

差距集中在 **流批计算 job 操控、消息队列、OLAP 查询、存储/元数据浏览**,这些恰是数据工程师每天敲 CLI 的高频场景。

---

## 3. 新增提议 — 12 个 frank-official 候选

按"现工作流会用到 + 不与现有 2 个 + 3 个占位重复 + 能 wrap 现成开源 CLI"筛选。

| # | name | category | description | value (具体场景) | inspiration / wrap 对象 | effort | priority |
|---|---|---|---|---|---|---|---|
| 1  | **flink-job-monitor**    | compute    | Flink JobManager REST 客户端 — 列作业 / 看 checkpoint / 触发 savepoint / 取 exception | StreamPark 装的作业出问题时,要绕过平台直接 ping JM,看上次 ckpt 时间 / 失败堆栈 / backpressure | Flink REST API + flink-cli (`bin/flink list/cancel/savepoint`) | M | **P0** |
| 2  | **kafka-ops**            | messaging  | Kafka topic / partition / consumer-group / lag — 列、看 offset、reset、produce 探针消息 | 排查"消费滞后"是日常: lag 多大、谁卡住、reset 到哪 offset | `kafka-topics.sh` / `kafka-consumer-groups.sh` / kcat | M | **P0** |
| 3  | **doris-ops**            | olap       | Doris / TCHouse-D 运维 — SHOW PROC、慢查、tablet 健康、BE/FE 状态 | 写完 Flink 入 Doris,要看 BE 报错 / tablet replica missing / compaction 落后 | MySQL CLI + Doris HTTP API; **复活注释占位** | M | **P0** |
| 4  | **hive-meta**            | metadata   | Hive Metastore 元数据查询 — DESC TABLE、partition 列表、location、lineage | Flink SQL CREATE TABLE 用了哪个 schema、partition 在不在、location 是不是脏 | HMS Thrift / `hive --service metatool` | M | P1 |
| 5  | **clickhouse-ops**       | olap       | ClickHouse 运维 — system.parts / merges / mutations / kill query | 历史日志或埋点表落 CK,合并卡死 / mutation 堆积时直接 system 表查 | `clickhouse-client` + system.* 表 | S | P1 |
| 6  | **redis-ops**            | cache      | Redis 运维 — 安全 SCAN、TTL 看、批量 set/delete、内存 top key | Flink state 之外业务 cache 滞留,需要 scan key 模式 + memory usage 排雷 | redis-cli; 关键是封装 SCAN 防止 KEYS * 打挂线上 | S | P1 |
| 7  | **dolphinscheduler-ops** | scheduler  | DolphinScheduler 任务运维 — 查 instance、重跑、暂停、看依赖图 | 调度失败要快速重跑、看上游为什么没出数据;**复活注释占位** | DS REST API (`/dolphinscheduler/projects/...`) | M | P1 |
| 8  | **hdfs-cli**             | storage    | HDFS / OSS 浏览 — ls / du / cat / 抽样 / 跨集群对账 | 跨 Hive / Iceberg 查 partition 文件大小、小文件统计 | `hdfs dfs` / `ossutil` 统一接口 | M | P1 |
| 9  | **iceberg-ops**          | datalake   | Iceberg 表运维 — snapshots、files、expire、compact、history | Flink 写 Iceberg 后看 metadata snapshot 数量、小文件、孤儿文件 | Iceberg Java CLI / spark-sql procedures | L | P2 |
| 10 | **es-ops**               | search     | Elasticsearch 索引管理 — _cat/indices, shard, reindex, alias 切换 | 业务搜索 / 日志栈, 索引膨胀 / shard 不均 / alias 滚动切换 | curl + `_cat/*` / cerebro 思路 | M | P2 |
| 11 | **feishu-read**          | doc        | 飞书文档/表格/Excel/PDF 抽取 — AI 读飞书页给上下文 | 项目需求文档在飞书,AI 当前看不到;**复活注释占位** | 飞书开放平台 OpenAPI (`/docx/v1/documents`) | M | P2 |
| 12 | **zk-etcd-ops**          | coord      | ZK + etcd 协调服务运维 — 看 znode/key 树、watch、session、quorum 健康 | Flink HA / Kafka controller / Curator 锁卡住要看 znode 内容 | `zkCli.sh` + `etcdctl`; 一个 skill 双引擎 | S | P2 |

### 与现有的关系

- **不重复** nacos-ops / streampark-ops。streampark-ops 是**平台层**(jar 上传/作业模板/启停),flink-job-monitor 是**JM 直连**(JM REST,绕开平台,救命场景)。
- **不重复** mcp-fetch / mcp-context7 等通用 MCP — 这些是 **领域** skill,带凭据 + 业务语义。
- 复活注释里的 3 个占位 (doris / feishu-read / dolphinscheduler) — 仓库一发就启用,占位本来就规划好了。

---

## 4. 优先级排序

### P0 (必须做, 3 个) — 数据工程师日常救火 90%

1. **flink-job-monitor** — Flink 生态主线, streampark 之外的 JM 直连
2. **kafka-ops** — 流处理上游, lag 排查日均高频
3. **doris-ops** — Flink 下游 OLAP 主力 (恰好已是注释占位)

### P1 (强烈推荐, 5 个) — 一个迭代内补齐

4. hive-meta
5. clickhouse-ops
6. redis-ops
7. dolphinscheduler-ops
8. hdfs-cli

### P2 (锦上添花, 4 个) — 下一轮再看

9. iceberg-ops
10. es-ops
11. feishu-read
12. zk-etcd-ops

---

## 5. 实施建议

### 5a. 可直接 wrap 现成 CLI (低成本, S/M)

- **kafka-ops** → 包 `kafka-topics.sh` + `kafka-consumer-groups.sh` + kcat (`brew install kcat`)
- **redis-ops** → 包 redis-cli,关键加 SCAN 防 KEYS * 误用
- **hdfs-cli** → 包 `hdfs dfs` + `ossutil`,统一前缀语义 `hdfs://` / `oss://`
- **clickhouse-ops** → 包 `clickhouse-client` + 内置 system.* 模板查询
- **zk-etcd-ops** → 包 `zkCli.sh` + `etcdctl`

这类 skill 主要价值在 **prompt 工程**(让 AI 知道何时该跑哪条 shell + 怎么解析输出)+ **凭据 / endpoint 隔离** (借助 frank `~/.frank/manifests/private-*.yaml` 注入 host)。**约半天到 1 天/个**。

### 5b. 需要写 REST 客户端 (中等, M)

- **flink-job-monitor** → Flink JM REST (`/jobs`, `/jobs/<id>/checkpoints`, `/jobs/<id>/exceptions`)
- **doris-ops** → MySQL 协议 + `/api/show_proc`、`/metrics`
- **hive-meta** → HMS Thrift(Python: `pyhive` / `hmsclient`)
- **dolphinscheduler-ops** → REST `/dolphinscheduler/projects/...`,token 认证
- **es-ops** → `_cat/*` JSON
- **feishu-read** → 飞书 OpenAPI

**约 1-2 天/个**。沿用 nacos-ops / streampark-ops 已经踩通的 Python + auth + config 模板,可批量复制。

### 5c. 需要新代码 / 较深生态绑定 (L)

- **iceberg-ops** → 依赖 spark-sql procedures 或 pyiceberg,环境装配麻烦。建议放 P2 待 Iceberg 在用户工作场景明确化后再做。

### 自含原则 (manifest 注释 §official)

每个 official skill 的**独立仓库**必须自带 setup (venv / npm i / 配置模板),frank 只 clone + symlink,**不解析 dependencies**。所有 12 个候选都符合此原则:每个对应单独 GitHub repo `hutiefang76/skills-<name>.git`,带 README + setup.sh + .skill.yaml。

---

## 6. 接入流程

新 skill 走完代码侧后,在 `builtin.yaml` **追加一行 entry** 即可,frank 不需要发版:

```yaml
  - name: kafka-ops
    description: 'Kafka topic / partition / consumer-group / lag 排查 — wraps kafka-topics.sh + kcat'
    source:
      type: git
      url: https://github.com/hutiefang76/skills-kafka-ops.git
      ref: main
    visibility: frank-official
```

用户端流程:

```bash
frank update                 # 拉最新 builtin.yaml
frank list                   # 看到 kafka-ops
frank install kafka-ops      # 装到 ~/.claude/skills/ ~/.codex/skills/ ~/.opencode/skills/
```

**注意点**:

1. builtin.yaml 是**编译期 embed** 进 frank binary 的,**改了要重 cargo build + 发版**。所以推荐:积攒一批新 skill 后,做一个 minor 版本一起放出,而不是改一行发一次。
2. `ref` 字段必须显写(nacos-ops 用 `master`,新仓默认 `main`,别忘了)。
3. URL 必须先**真发** GitHub repo 后再加(不要重蹈 doris-ops 注释占位却 404 的覆辙;新 skill **先发仓再加 yaml**)。
4. visibility 都是 `frank-official`(也兼容老 `frank-own`)。
5. 全部加 `target_platforms` 兼容性:大数据 skill 主要是 Python + REST,默认装 claude/codex/opencode 三平台均可不写 (走默认全平台)。

---

## 7. 与 frank-memory 的边界 (POSITION.md 对照)

| 维度 | frank skills (本提议) | frank-memory |
|---|---|---|
| 治理对象 | **工具能力** — 让 AI 学会调用 Kafka / Flink JM / Doris | **用户数据** — 跨设备共享 "我是谁" 的事实 |
| 数据流 | 静态文件 (skill 目录 + slash 命令) → 三平台 skills/ 目录 | 动态: extract → store → semantic recall |
| 风险点 | 凭据注入 (skill 跑业务 API,带 host / token / cookie) | 数据漂移 (LWW / 跨设备同步) |
| 阶段 | P0 已落地 (install/uninstall/list 跑通) | P5 进行中 (Qdrant 部署 + extract 抽取) |
| 关系 | 本提议**完全在 skill 治理框架内**,**不动 memory** | 跟 skill 互不依赖 |

新增 12 个 official skill **不需要**任何 memory 改动:它们只用 frank P0 已有的 install/uninstall/list/enable/disable 能力。

---

## 8. 结论与下一步

- **量化差距**: 用户期望 official 系列 8-15 个;现 2 个;本提议补 12 个 (含复活 3 个占位)。
- **P0 三件**: flink-job-monitor / kafka-ops / doris-ops — 任何 Flink 数据工程团队 90% 救火场景的工具底座。
- **下一步 (用户 review 通过后)**:
  1. 用户挑出 P0 三个里**先做哪一个**,作者建议从 **kafka-ops** 起手 (wrap 现成 CLI,1 天可 demo,验证 official skill 模板成熟度)。
  2. 在 hutiefang76 名下创建对应空仓 (`skills-<name>`)。
  3. 走 nacos-ops / streampark-ops 已验证的模板:`SKILL.md` + `setup.sh` + Python `__main__.py` + `config/` 模板。
  4. 仓发出来后再改 `builtin.yaml`,跟随下一个 frank minor 发版统一带出。

---

*Draft v1.0 — 待用户拍板优先级 / 增删候选。*
