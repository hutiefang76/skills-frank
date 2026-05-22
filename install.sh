#!/usr/bin/env bash
# frank — 一键安装脚本 (零依赖空环境也能跑)
#
# 用法:
#   curl -fsSL https://raw.githubusercontent.com/hutiefang76/skills-frank/main/install.sh | bash
#
# 行为:
#   1. 检测代理 (HTTP_PROXY / HTTPS_PROXY / clash 默认 127.0.0.1:7897)
#   2. 检测 / 安装 Rust toolchain (rustup 自动)
#   3. 配置 cargo 镜像 (中科大 ustc; 仅当未配过)
#   4. clone + cargo install --path crates/frank-cli (走 git2 vendored, 无系统依赖)
#   5. 写 ~/.frank/ + state.json 初始化
#   6. 打印 frank --help / frank doctor 引导
#
# 失败模式: set -e + 任意命令 fail 立刻退, 打印 frank-doctor 引导

set -eo pipefail

readonly REPO_OWNER="hutiefang76"
readonly REPO_NAME="skills-frank"
readonly REPO_URL="https://github.com/${REPO_OWNER}/${REPO_NAME}.git"
readonly RELEASES_API="https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/releases/latest"
readonly RELEASES_DL="https://github.com/${REPO_OWNER}/${REPO_NAME}/releases"
readonly INSTALL_DIR="${FRANK_INSTALL_DIR:-$HOME/.frank-src}"
readonly BIN_DIR="${FRANK_BIN_DIR:-$HOME/.local/bin}"

# ─── 着色 + log ─────────────────────────────────────────────
c_green=$'\033[0;32m'; c_yellow=$'\033[1;33m'; c_red=$'\033[0;31m'; c_blue=$'\033[0;34m'; c_reset=$'\033[0m'
say()  { printf '%s→%s %s\n' "$c_blue"   "$c_reset" "$*"; }
ok()   { printf '%s✓%s %s\n' "$c_green"  "$c_reset" "$*"; }
warn() { printf '%s!%s %s\n' "$c_yellow" "$c_reset" "$*" >&2; }
die()  { printf '%s✗%s %s\n' "$c_red"    "$c_reset" "$*" >&2; exit 1; }

# ─── 1. 代理自动检测 ────────────────────────────────────────
detect_proxy() {
  if [[ -n "${HTTPS_PROXY:-}${https_proxy:-}" ]]; then
    ok "检测到代理 env: ${HTTPS_PROXY:-$https_proxy}"
    return
  fi
  # 试常见 clash / v2ray 端口
  for port in 7897 7890 1087 7891; do
    if curl -sf -m 2 --proxy "http://127.0.0.1:$port" https://www.google.com -o /dev/null 2>/dev/null; then
      export HTTPS_PROXY="http://127.0.0.1:$port"
      export HTTP_PROXY="http://127.0.0.1:$port"
      ok "检测到本机代理 127.0.0.1:$port, 已注入 env"
      return
    fi
  done
  warn "未检测到代理. GitHub / crates.io 拉取可能慢, 可手动 export HTTPS_PROXY=..."
}

# ─── 2. Rust toolchain ──────────────────────────────────────
ensure_rust() {
  if command -v cargo >/dev/null 2>&1; then
    local v
    v=$(cargo --version | awk '{print $2}')
    ok "Rust 已装: cargo $v"
    return
  fi
  say "Rust 未装, 走 rustup 自动安装 (--default-toolchain stable)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y --default-toolchain stable
  # rustup 装完 PATH 没生效, source env
  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  fi
  command -v cargo >/dev/null 2>&1 || die "rustup 装完但 cargo 还找不到. 请 source ~/.cargo/env 后重跑"
  ok "Rust 装好: $(cargo --version)"
}

# ─── 3. cargo 镜像 (中国大陆友好) ──────────────────────────
ensure_cargo_mirror() {
  local cfg="$HOME/.cargo/config.toml"
  if [[ -f "$cfg" ]] && grep -q '\[source.crates-io\]' "$cfg" 2>/dev/null; then
    ok "cargo 镜像已配 (跳过)"
    return
  fi
  # 检测网络: 5s 内连到 crates.io 算 OK
  if curl -sf -m 5 https://crates.io -o /dev/null 2>/dev/null; then
    ok "crates.io 直连可达 (跳过镜像)"
    return
  fi
  # 国内慢时, 默认 *不* 修改 ~/.cargo/config.toml — 让用户主动选择
  warn "crates.io 慢. 如需镜像加速请手动加 (中科大 ustc 推荐):"
  cat <<'TIP'

  cat >> ~/.cargo/config.toml <<'CFG'
  [source.crates-io]
  replace-with = "ustc"
  [source.ustc]
  registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
  CFG

TIP
  warn "或设 FRANK_INSTALL_AUTO_MIRROR=1 让 install.sh 自动配 (默认不动)"
  if [[ "${FRANK_INSTALL_AUTO_MIRROR:-0}" == "1" ]]; then
    mkdir -p "$HOME/.cargo"
    cat >> "$cfg" <<'CFG'

# 由 frank install.sh 添加 (国内网络加速)
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
CFG
    ok "cargo 镜像配好 ($cfg)"
  fi
}

# ─── 4a. 优先: 下预编译 binary ──────────────────────────────
# 检测 OS + arch 对应 release.yml 的 target triple, curl latest release
# 失败 (404 / 无网络 / 没匹配 target) 自动 fallback 到 4b cargo build
detect_target() {
  local os arch
  case "$(uname -s)" in
    Darwin)  os=apple-darwin ;;
    Linux)   os=unknown-linux-gnu ;;
    MINGW*|MSYS*|CYGWIN*) os=pc-windows-msvc ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=aarch64 ;;
    *) return 1 ;;
  esac
  echo "${arch}-${os}"
}

try_install_binary() {
  local target
  target=$(detect_target) || { warn "未识别的 OS/arch, 跳过 binary, fallback build"; return 1; }
  say "尝试下载预编译 binary (target: $target)"

  # 拉 latest release json, 提取该 target 的 .tar.gz / .zip 直链
  local tag asset_url ext
  tag=$(curl -fsSL "$RELEASES_API" 2>/dev/null | grep -oE '"tag_name": *"[^"]+"' | head -1 | cut -d'"' -f4)
  if [[ -z "$tag" ]]; then
    warn "GitHub Release API 不可达 (或本仓还未发布); fallback build"
    return 1
  fi
  ok "最新 release tag: $tag"

  if [[ "$target" == *windows* ]]; then ext="zip"; else ext="tar.gz"; fi
  asset_url="${RELEASES_DL}/download/${tag}/frank-${tag}-${target}.${ext}"

  local tmp; tmp=$(mktemp -d)
  trap "rm -rf '$tmp'" RETURN
  if ! curl -fsSL "$asset_url" -o "$tmp/frank.${ext}"; then
    warn "下载 $asset_url 失败 (该 target 可能没出包); fallback build"
    return 1
  fi

  # 解包
  if [[ "$ext" == "zip" ]]; then
    (cd "$tmp" && unzip -q frank.zip)
  else
    (cd "$tmp" && tar xzf frank.tar.gz)
  fi
  local binname=frank
  [[ "$target" == *windows* ]] && binname=frank.exe
  if [[ ! -f "$tmp/$binname" ]]; then
    warn "解包后未找到 $binname (archive 结构异常); fallback build"
    return 1
  fi

  mkdir -p "$BIN_DIR"
  install -m 755 "$tmp/$binname" "$BIN_DIR/$binname"
  ok "binary 安装到 $BIN_DIR/$binname"

  # PATH 提醒
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
      warn "$BIN_DIR 不在 PATH; 加到 shell rc:"
      echo "    echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc   # 或 ~/.bashrc"
      ;;
  esac
  return 0
}

# ─── 4b. fallback: clone + cargo build ─────────────────────
install_frank_from_source() {
  # 需要 cargo, 由 ensure_rust 保证
  if [[ -d "$INSTALL_DIR/.git" ]]; then
    say "本地已有源码 ($INSTALL_DIR), git pull 更新"
    (cd "$INSTALL_DIR" && git pull --rebase --autostash) || warn "git pull 失败, 继续用现有版本"
  else
    say "git clone $REPO_URL → $INSTALL_DIR"
    git clone --depth 1 "$REPO_URL" "$INSTALL_DIR"
  fi

  say "cargo install (release, 走 git2 vendored, 首次 ~3 min)"
  cargo install --path "$INSTALL_DIR/crates/frank-cli" --locked
  ok "frank 装好: $(command -v frank)"
}

install_frank() {
  if try_install_binary; then
    return
  fi
  ensure_rust
  ensure_cargo_mirror
  install_frank_from_source
}

# ─── 5. 初始化 ~/.frank ─────────────────────────────────────
init_frank_home() {
  mkdir -p "$HOME/.frank/manifests"
  if [[ ! -f "$HOME/.frank/state.json" ]]; then
    cat > "$HOME/.frank/state.json" <<'JSON'
{
  "schema_version": 1,
  "profile": "personal",
  "skills": {}
}
JSON
    ok "~/.frank/state.json 初始化"
  fi
}

# ─── main ───────────────────────────────────────────────────
main() {
  echo
  echo "  frank — AI 工具链治理平台 一键安装"
  echo "  https://github.com/hutiefang76/skills-frank"
  echo

  detect_proxy
  # 注: ensure_rust / ensure_cargo_mirror 只在 binary 下载失败、需要本地 build 时才调用
  # (install_frank 内部判断), 避免给纯下载用户拉 Rust toolchain.
  install_frank
  init_frank_home

  echo
  ok "全部完成! 试一下:"
  echo "    frank --help"
  echo "    frank doctor          # 检测环境完整度"
  echo "    frank scan            # 扫本机三平台已装 skills"
  echo
  echo "  分布式记忆 (需 sync-agent 公网 token):"
  echo "    export FRANK_SYNC_AGENT_URL=https://frank.hutiefang.com"
  echo "    echo \$YOUR_TOKEN > ~/.frank/.token && chmod 600 ~/.frank/.token"
  echo "    frank memory healthz"
}

# 只在直接执行时跑 main; source 时 (例如调试单个函数) 不自动启动
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
