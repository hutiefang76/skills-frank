#!/usr/bin/env bash
# frank 一行彻底卸载脚本 (v0.7.2+)
#
# 用法 (任一):
#   ./scripts/uninstall-frank.sh                  # 交互式 (推荐, 每步问)
#   ./scripts/uninstall-frank.sh --yes            # 全部自动 yes
#   ./scripts/uninstall-frank.sh --keep-config    # 保留 ~/.frank/ (token + state)
#
# 远程一行:
#   curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/scripts/uninstall-frank.sh | bash -s -- --yes
#
# 为什么不放进 brew uninstall?
#   Homebrew 设计上 brew uninstall 只删 Cellar binary, 不动用户数据 (跟 ollama/postgres
#   一致). frank 的"用户数据" = 三平台 skill symlink + ~/.claude.json mcpServers 注入 +
#   ~/.frank/ — brew 设计上不该动. 所以提供这个脚本做完整清理.

set -euo pipefail

AUTO_YES=false
KEEP_CONFIG=false
for arg in "$@"; do
  case "$arg" in
    --yes|-y) AUTO_YES=true ;;
    --keep-config) KEEP_CONFIG=true ;;
    -h|--help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $arg"; exit 1 ;;
  esac
done

confirm() {
  if [ "$AUTO_YES" = true ]; then
    echo "  → $1 (--yes, auto)"
    return 0
  fi
  printf "  ? %s [y/N]: " "$1"
  read -r ans
  [ "$ans" = "y" ] || [ "$ans" = "Y" ]
}

step() {
  printf "\n\033[1;36m▸\033[0m %s\n" "$1"
}

ok() {
  printf "  \033[1;32m✓\033[0m %s\n" "$1"
}

skip() {
  printf "  \033[1;33m-\033[0m %s\n" "$1"
}

# ─── 检测 frank 是否装着 ──────────────────────────────────────────
if ! command -v frank >/dev/null 2>&1; then
  printf "\033[1;33m!\033[0m frank not in PATH — 假定已经卸过, 仅清残留\n"
  HAS_FRANK=false
else
  HAS_FRANK=true
  FRANK_VERSION=$(frank --version 2>/dev/null | head -1)
  echo "检测到 $FRANK_VERSION"
fi

# ─── Step 1: frank cleanup (skill + MCP + cache) ─────────────────
step "Step 1/4: 清三平台 skill / MCP / cache"
if [ "$HAS_FRANK" = true ]; then
  if confirm "跑 frank cleanup (清所有 skill + MCP 注入 + git cache)"; then
    frank cleanup 2>&1 | sed 's/^/    /'
    ok "frank-managed skill / MCP / cache 已清"
  else
    skip "跳过 frank cleanup (你后面手动卸三平台的 frank-* skill 文件夹)"
  fi
else
  skip "frank binary 不在, 跳"
fi

# ─── Step 2: stop brew services ──────────────────────────────────
step "Step 2/4: 停 brew services frank"
if command -v brew >/dev/null 2>&1; then
  if brew services list 2>/dev/null | grep -q "^frank"; then
    if confirm "跑 brew services stop frank"; then
      brew services stop frank 2>&1 | sed 's/^/    /'
      ok "daemon stopped"
    else
      skip "跳过 stop service"
    fi
  else
    skip "brew services 没注册 frank (可能没用 brew 装的)"
  fi
else
  skip "brew not found, 跳"
fi

# ─── Step 3: brew uninstall ──────────────────────────────────────
step "Step 3/4: brew uninstall frank (删 binary)"
if command -v brew >/dev/null 2>&1 && brew list --formula 2>/dev/null | grep -q "^frank$"; then
  if confirm "跑 brew uninstall frank"; then
    brew uninstall frank 2>&1 | sed 's/^/    /'
    ok "binary 删了 + brew tap 仍保留 (跑 brew untap hutiefang76/frank 彻底清)"
  else
    skip "跳过 brew uninstall"
  fi
else
  skip "frank 不是 brew 装的 (跳过这步)"
fi

# ─── Step 4: ~/.frank/ 用户数据 ──────────────────────────────────
step "Step 4/4: ~/.frank/ 用户数据 (token / state / logs / ai_history)"
if [ -d "$HOME/.frank" ]; then
  if [ "$KEEP_CONFIG" = true ]; then
    skip "--keep-config, 保留 ~/.frank/ (token / state 留, 以便重装继续用)"
  elif confirm "rm -rf ~/.frank/ (含 sync-agent token, AI ask 历史, state.json)"; then
    rm -rf "$HOME/.frank"
    ok "~/.frank/ 已删"
  else
    skip "保留 ~/.frank/"
  fi
else
  skip "~/.frank/ 不存在, 跳"
fi

# ─── 总结 ────────────────────────────────────────────────────────
printf "\n\033[1;32m✓ 卸载流程完成\033[0m\n"
if [ "$KEEP_CONFIG" = true ] || [ -d "$HOME/.frank" ]; then
  echo "  ~/.frank/ 保留: 重装后 frank login / state 直接接管."
fi
if command -v brew >/dev/null 2>&1 && brew tap 2>/dev/null | grep -q "^hutiefang76/frank$"; then
  printf "\033[1;36m提示\033[0m: brew tap 仍注册 (brew untap hutiefang76/frank 彻底清)\n"
fi
