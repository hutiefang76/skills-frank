---
name: frank-ask-gemini
description: 把当前用户问题原话转发给 gemini CLI (Google), 拿回答原样返回. 触发: 用户输入 "/frank:ask:gemini <prompt>", "/frank-ask-gemini <prompt>", "/frank:ask:google <prompt>", "ask gemini", "问 google", "frank 转 gemini" 等明确指定要 gemini/google 回答的话.
---

# frank-ask-gemini

用户**明确要 gemini / google 的视角**时, 用 Bash 工具调:

```bash
frank ai ask --to gemini "<用户的 prompt 原话>"
```

**约定**: 不修改 prompt, 模型由 gemini CLI 自配置, prompt 含特殊字符 heredoc.
