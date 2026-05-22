---
name: frank-ask-gpt
description: 把当前用户问题原话转发给 codex CLI (gpt-5.5 Plus 订阅), 拿回答原样返回. 触发: 用户输入 "/frank:ask:gpt <prompt>", "/frank-ask-gpt <prompt>", "ask gpt:", "问 gpt", "frank 转 gpt" 等任何明确指定要 gpt/codex 回答的话.
---

# frank-ask-gpt

用户**明确要 gpt / codex 的视角**时, 用 Bash 工具调:

```bash
frank ai ask --to gpt "<用户的 prompt 原话>"
```

**约定**:
- 不修改 prompt (不翻译/不优化/不加 system prompt)
- prompt 含特殊字符用 heredoc:
  ```bash
  frank ai ask --to gpt "$(cat <<'EOF'
  <用户原文>
  EOF
  )"
  ```
- stdout 原样返回, 不加 "GPT 说:" 这种修饰
- 默认 `--timeout 300` (codex high-reasoning 慢)
- 模型由 codex CLI 自身配置决定 (用户已配 gpt-5.5)
