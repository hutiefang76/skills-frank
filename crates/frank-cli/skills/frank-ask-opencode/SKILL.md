---
name: frank-ask-opencode
description: 把当前用户问题原话转发给 opencode CLI (Go 套餐, 默认 qwen3.6+), 拿回答原样返回. 触发: 用户输入 "/frank:ask:opencode <prompt>", "/frank-ask-opencode <prompt>", "/frank:ask:qwen <prompt>", "ask opencode", "问 qwen", "frank 转 opencode" 等明确指定要 opencode/qwen 回答的话.
---

# frank-ask-opencode

用户**明确要 opencode / qwen 的视角**时, 用 Bash 工具调:

```bash
frank ai ask --to opencode "<用户的 prompt 原话>"
```

**约定**:
- 不修改 prompt
- 模型由 opencode CLI 自身配置决定 (用户已订 Go 套餐, 默认 qwen3.6+)
- prompt 含特殊字符用 heredoc
