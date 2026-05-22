---
name: frank-ask-claude
description: 把当前用户问题原话转发给 claude CLI (Pro/Max 订阅 opus 默认), 拿回答原样返回. 触发: 用户输入 "/frank:ask:claude <prompt>", "/frank-ask-claude <prompt>", "ask claude:", "问 claude", "frank 转 claude", "用 opus 看下" 等明确指定要 claude/opus 回答的话.
---

# frank-ask-claude

用户**明确要 claude / opus 的视角**时, 用 Bash 工具调:

```bash
frank ai ask --to claude "<用户的 prompt 原话>"
```

**约定**:
- 不修改 prompt
- 模型由 claude CLI 自身配置决定 (用户已配 opus/sonnet/haiku 默认)
- 失败常见原因: `claude` CLI 没登录 (跑 `claude setup-token` 一次)
- prompt 含特殊字符用 heredoc

**变体识别**: 用户说 "用 opus / sonnet / haiku" 时也走这条 — claude CLI 自己路由到对应模型, frank 不动 CLI 参数.
