#!/usr/bin/env bash
#
# tests/refresh_skills_e2e.sh — v0.10.8 D6 端到端真测
#
# 测试 `frank refresh-skills` 按 `~/.claude/settings.json` 的 model 字段动态生成
# slash command skill, 且 model 从配置里删除时旧 skill 也被清。
#
# # 怎么跑
#
#   bash tests/refresh_skills_e2e.sh
#
# 退出码 0 = 全过, 非 0 = 哪步失败 (脚本会打哪步红字).
#
# # 隔离
#
# 用临时 HOME (tempdir) 跑, 不污染用户真实 ~/.claude/settings.json 和 ~/.claude/skills/.
# 用 cargo build --release + 直接调 binary, 避免每步 cargo run 慢编译.

set -euo pipefail

# ─── 1. 准备 ───────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[0;33m'
NC=$'\033[0m'

step() { echo "${YELLOW}→${NC} $*"; }
pass() { echo "${GREEN}✓${NC} $*"; }
fail() { echo "${RED}✗${NC} $*"; exit 1; }

# ─── 2. 编 binary (debug 够用, 比 release 快) ─────────────
step "编 frank binary"
cargo build --bin frank --quiet 2>&1 | tail -5
FRANK="$REPO_ROOT/target/debug/frank"
[ -x "$FRANK" ] || fail "binary 没编出来: $FRANK"
pass "binary 在 $FRANK"

# ─── 3. 临时 HOME 隔离 ─────────────────────────────────
TMP_HOME=$(mktemp -d)
trap 'rm -rf "$TMP_HOME"' EXIT
export HOME="$TMP_HOME"
step "临时 HOME = $TMP_HOME"

# 准备目录结构
mkdir -p "$HOME/.claude/skills"
mkdir -p "$HOME/.frank"

# ─── 4. 场景 1: 配 kimi-k2.5 → refresh → skill 生成 ──────
step "场景 1: 在 ~/.claude/settings.json 写 model: kimi-k2.5"
cat > "$HOME/.claude/settings.json" <<EOF
{
  "model": "kimi-k2.5",
  "theme": "dark"
}
EOF

step "跑 frank refresh-skills"
"$FRANK" refresh-skills 2>&1 | tee "$TMP_HOME/refresh1.log"

step "检查 ~/.claude/skills/frank-ask-claude-kimi-k2-5/SKILL.md 存在"
SKILL_PATH="$HOME/.claude/skills/frank-ask-claude-kimi-k2-5/SKILL.md"
[ -f "$SKILL_PATH" ] || fail "SKILL.md 没生成: $SKILL_PATH"
pass "SKILL.md 在"

step "检查 SKILL.md 含 frank ai ask --to claude --model kimi-k2.5"
if grep -q "frank ai ask --to claude" "$SKILL_PATH" && grep -q "model kimi-k2.5" "$SKILL_PATH"; then
    pass "SKILL.md 内容正确 (含 --to claude + --model kimi-k2.5)"
else
    cat "$SKILL_PATH"
    fail "SKILL.md 内容不对"
fi

# ─── 5. 场景 2: 配换成 sonnet → refresh → stale 清掉 ──────
step "场景 2: 把 ~/.claude/settings.json 改成 model: sonnet (kimi 应该被清)"
cat > "$HOME/.claude/settings.json" <<EOF
{
  "model": "sonnet"
}
EOF

step "再跑 frank refresh-skills"
"$FRANK" refresh-skills 2>&1 | tee "$TMP_HOME/refresh2.log"

step "检查 sonnet skill 在"
SONNET_PATH="$HOME/.claude/skills/frank-ask-claude-sonnet/SKILL.md"
[ -f "$SONNET_PATH" ] || fail "sonnet SKILL.md 没生成: $SONNET_PATH"
pass "sonnet skill 在 $SONNET_PATH"

step "检查 kimi-k2-5 skill 被清掉"
KIMI_DIR="$HOME/.claude/skills/frank-ask-claude-kimi-k2-5"
if [ -d "$KIMI_DIR" ]; then
    fail "kimi 没被清: $KIMI_DIR 还在"
else
    pass "kimi-k2-5 stale skill 已清"
fi

# ─── 6. 场景 3: 删 model 字段 → refresh → 走兜底 (sonnet 仍在) ──
step "场景 3: 删 model 字段 (走 builtin alias 兜底)"
cat > "$HOME/.claude/settings.json" <<EOF
{
  "theme": "dark"
}
EOF

step "再跑 frank refresh-skills"
"$FRANK" refresh-skills 2>&1 | tee "$TMP_HOME/refresh3.log"

step "检查兜底 alias 至少装了 sonnet (BUILTIN_ALIASES claude 第一个)"
if [ -d "$HOME/.claude/skills/frank-ask-claude-sonnet" ]; then
    pass "BUILTIN_ALIASES 兜底生效, sonnet skill 在"
else
    ls "$HOME/.claude/skills/" 2>&1
    fail "兜底没生效 — sonnet skill 不在"
fi

# ─── 7. 场景 4: --dry-run 不实际写 ────────────────────────
step "场景 4: --dry-run 不该改任何文件"
# 配一个新 model, dry-run 跑
cat > "$HOME/.claude/settings.json" <<EOF
{
  "model": "opus"
}
EOF
OPUS_PATH="$HOME/.claude/skills/frank-ask-claude-opus/SKILL.md"
rm -rf "$HOME/.claude/skills/frank-ask-claude-opus"  # 确保不存在

"$FRANK" refresh-skills --dry-run 2>&1 | tee "$TMP_HOME/refresh4.log"

if [ -f "$OPUS_PATH" ]; then
    fail "--dry-run 还是写了文件: $OPUS_PATH 存在"
else
    pass "--dry-run 没写文件 (opus skill 不存在, 符合预期)"
fi

# ─── 8. 场景 5: 用户自己 skill 不被动 ─────────────────────
step "场景 5: 用户自定义 skill 不该被 clean 误删"
mkdir -p "$HOME/.claude/skills/my-own-skill"
echo "user content" > "$HOME/.claude/skills/my-own-skill/SKILL.md"

"$FRANK" refresh-skills 2>&1 | tee "$TMP_HOME/refresh5.log"

if [ -f "$HOME/.claude/skills/my-own-skill/SKILL.md" ]; then
    pass "用户 skill 没被动"
else
    fail "用户 skill 被误删!"
fi

# ─── 9. 收尾 ─────────────────────────────────────────────
echo
pass "全 5 个场景全过"
echo
echo "生成的 skill:"
ls "$HOME/.claude/skills/" | sed 's/^/  /'
