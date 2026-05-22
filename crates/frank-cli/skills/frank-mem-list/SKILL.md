---
name: frank-mem-list
description: 列出用户分布式记忆 (frank-memory). 触发: 用户输入 "/frank:mem:list", "/frank:memory:list", "/frank-mem-list", "frank 列记忆", "我的记忆有什么", "frank memory 列一下" 等明确要看自己记忆条目的话.
---

# frank-mem-list

用户**想看自己已存的记忆**时, 用 Bash 工具调:

```bash
frank memory list --user <user_id> --limit 20
```

**默认 user_id**: 如果用户没明示, 用本机 `whoami` 或 `hutiefang` (项目作者). 让用户用 `--user X` 显式切换其他 scope.

输出表格直接给用户看 (frank memory list 已经表格化, 不要重新格式化).

# 故障

- "no record in scope" → 没存过 / scope 不对, 引导 `frank:mem:add` 先加
- "sync-agent unreachable" → 服务端挂了, 跑 `frank doctor` 看
