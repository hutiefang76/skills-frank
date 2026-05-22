---
name: frank-mem-search
description: 语义检索用户分布式记忆 (frank-memory). 触发: 用户输入 "/frank:mem:search <query>", "/frank:memory:search <query>", "/frank-mem-search <query>", "frank 搜记忆 <query>", "我记得有 <query> 相关的东西" 等明确想从记忆中找事的话.
---

# frank-mem-search

用户**想从记忆中查询**时, 用 Bash 工具调:

```bash
frank memory search "<query>" --user <user_id> --limit 10
```

**约定**:
- query 用引号包(防 shell 拆词)
- 默认 user_id = `whoami` 或 `hutiefang`
- 把 `frank memory search` 的输出原样返回给用户
- 若 "no match" 但用户问题明显有相关性,提示用户先用 `/frank:mem:add` 加几条
