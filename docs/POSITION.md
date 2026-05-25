# frank-memory 产品定位(2026-05-25 用户拍板版)

> 防止 AI 协作者再次跑偏的固化文档。所有 v0.10.6+ 计划必须对照本文档校验。

## 🎯 第 0 优先级:存在动因

**多机同步** — 跨设备共享 skills + memory,新机器装 frank 立刻到位,**不用每台机器重新教 AI 我是谁**。

`device token 解耦中转站` 是这个动因的**特化场景**:共享账号下账号级记忆不安全,所以走自己 device token。

## 📊 13 维度对照表(用户 2026-05-25 表态)

| # | 维度 | 别人(基线) | frank 现状 | 决策 | 阶段 |
|---|---|---|---|---|---|
| 1 | **存储** | mem0/Letta/Graphiti(远程为主)<br>cognee(embedded 但 Python) | Qdrant 远程 | ⚠️ **倒过来** — LanceDB 本地主存,Qdrant 仅同步 | v0.11 |
| 2 | **抽取** | 固定 LLM(mem0=OpenAI/Claude) | 固定 Claude Haiku | ⚠️ **用用户当前 AI** — codex 用户走 codex 抽 | v0.11 |
| 3 | **隔离** | scope filter | user/agent/session | ✅ 不动 | 已有 |
| 4 | **多路召回** | mem0(语义+BM25+实体)、Zep(语义+BM25+图) | 单路向量 | ⚠️ **4 路并行 + RRF** | v0.11 |
| 5 | **跨设备同步** | mem0/Letta cloud | sync-agent + tx | 强化(LWW 冲突解决) | v0.12 |
| 6 | **记忆分类** | LangMem(semantic/episodic/procedural) | flat | ⚠️ **三类全做** | v0.11 |
| 7 | **session 分层** | Letta(Core/Recall/Archival) | session_id filter | ⚠️ **三层做完** | v0.11 |
| 8 | **Graph** | mem0v2/Graphiti/cognee | 无 | ⚠️ **轻量级 + 本地 haiku/小模型,环境检测** | v0.12 |
| 9 | **时间维度** | Graphiti bi-temporal | 仅 created_at | 后补 | v0.13 |
| 10 | **接入方式** | SDK(mem0 Python)、MCP server(官方) | CLI + REST | MCP 兼容 + 钩子(零 token) | v0.11+v0.12 |
| 11 | **跨 provider** | 各家 SDK 绑 1 个 AI | claude+codex+gemini+opencode | ✅ 不动 | 已有 |
| 12 | **可观测** | mem0 dashboard / Letta UI | v0.10.5 token+latency stderr | ⚠️ **加召回路径 / extractor / cache 命中** | v0.11 |
| 13 | **零依赖装** | mem0/Letta 需 Python | cargo install 单 binary | ✅ 不动 | 已有 |

## 🔑 frank 的 3 个独家定位

| # | 定位 | 别人有? | frank 怎么做 |
|---|---|---|---|
| 1 | **跨 AI provider 工具链统一**(不是 SDK 绑一个 AI) | ❌ mem0/Letta 都是 Python SDK 绑一个 AI | claude+codex+gemini+opencode 同一套记忆 |
| 2 | **device token 解耦中转站 / 共享账号**(多机同步动因的特化) | ❌ 全行业假设账号即身份 | per-device token,user_id 后端反查 |
| 3 | **Rust 嵌入式单 binary**(cargo install / brew install 一行) | ❌ Python+server 全员 | 4.7MB binary 静态链接 |

## 🆕 用户 P0/P1 新增强需求(2026-05-25)

| # | 需求 | 优先级 | 阶段 |
|---|---|---|---|
| 3.1 | **ask 动态加载用户配的模型**(层级 1 工具 + 层级 2 模型,opencode 用户自配的也列) | 🔥🔥🔥 P0 | v0.10.7 |
| 3.2 | **同机多 cli 窗口 ask 并发**(CCB 不支持) | 🔥🔥🔥 P0 | v0.10.7 验证 |
| 3.3 | **可视化查看 ai 历史**(GuDaStudio 不支持) | 🔥🔥 P1 | v0.10.7(Web UI tab) |
| 3.4 | **管理已问的问题**(history + delete/export) | 🔥🔥 P1 | v0.10.7(CLI + UI) |
| 3.5 | ~~token 预算预估~~ | ❌ 用户撤 | 不做 |

## ❌ 撤回项(我之前误导)

| # | 撤回 | 原因 |
|---|---|---|
| A | **自动淘汰策略**(LFU+LRU+Agentic) | 行业 0/6 家做。原因:用户记忆增长慢、删错代价高、存储便宜。改为 **手动 `frank memory cleanup`** + 检索时**软降权**(不删,Rerank 后置) |
| B | ~~v0.10.5 含 pricing 表算 cost~~ | 中转站价 ≠ 官方 + 2026 漂移频繁。已撤,改为只显 token+latency |
| C | ~~v0.10.8 token 预算预估~~ | 用户拍板"只显示已花,不预估" |

## 📅 5 阶段路标(2026-05-25 最终版)

| 版本 | 工期 | 主轴 |
|---|---|---|
| **v0.10.6** | 3d | 自动接管 lifecycle(install/enable→disable mcp_memory,uninstall→restore)+ POSITION.md ✅(本文档) |
| **v0.10.7** | 4-5d | **多模型动态加载 + 多窗口并发验证 + ai history 管理(CLI+UI)** |
| **v0.11.0** | 7-10d | 本地 LanceDB 主存倒置 + 多路召回 4 路 + extractor auto-detect + **三类记忆** + **三层 session** + 可观测细化 + 软降权检索 + 手动 cleanup |
| **v0.12.0** | 7-10d | 多机同步强化(LWW)+ device token + 轻量 Graph(haiku/本地小模型 + 环境检测)+ MCP server 协议兼容 + PostToolUse 钩子 |
| **v0.13+** | 远期 | 时间维度 bi-temporal + Windows 真测 |

## 🛡 校验规则(给未来的 AI 协作者)

1. **任何新功能提议必须对照本文档 13 维度 + 3 独家定位 + 撤回项**
2. **"行业 X 都有所以我们也得有"是基线,不是创新点** — 创新只在 3 个独家定位
3. **"我自己拍脑袋"的方案要先调研 4+ 开源系统是否做、为什么做 / 不做**
4. **撤回项不能复活**(除非有新证据推翻原撤回理由)
5. **用户痛点 > 工程美学** — pricing 表 / 自动淘汰 / Gemini 全平台支持等"全面性"提议必须先问用户

---

*最后更新: 2026-05-25(v0.10.6 启动前)。本文件 commit 进 main,任何改动必须 PR + 用户 review。*
