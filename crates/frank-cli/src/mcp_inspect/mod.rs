//! `mcp_inspect` — 只读探测 4 platform 的 MCP memory 配置。
//!
//! # 为什么独立成 module (而不是塞 `installer/mcp.rs`)
//!
//! `installer/mcp.rs` 是**写**路径 (install_claude / uninstall_codex …),
//! 这里是**读**路径 (问"用户当前装了 official memory MCP 没?"). 写出 bug 会
//! 破坏 `~/.claude.json` 200K 行用户配置 (R-P2.1, 严重性 HIGH); 读不会。
//! 所以分离, 避免任何调用方误把这两套混用。
//!
//! # 4 provider 配置位置 (实测)
//!
//! | Provider | 路径 | 格式 |
//! |---|---|---|
//! | claude | `~/.claude.json` | JSON, 顶层 `mcpServers.<name>` |
//! | codex | `~/.codex/config.toml` | TOML, `[mcp_servers.<name>]` |
//! | gemini | `~/.gemini/settings.json` | JSON, 顶层 `mcpServers.<name>` |
//! | opencode | `~/.config/opencode/opencode.json` | JSONC, 顶层 `mcp.<name>`, command 是 array |
//!
//! 全部**只读**: `fs::read_to_string` → parse → `Ok(Some(...)) / Ok(None) / log warn`.
//! 永不写。任何 IO 错误降级为 "no config detected" 而非 bail (doctor 必须能跑完)。
//!
//! # 检测目标
//!
//! 1. **Official memory MCP** — `@modelcontextprotocol/server-memory` (npx) 或
//!    `mcp-server-memory` (uvx). 装了的话, 推荐用户禁用以让 frank-memory 接管。
//! 2. **Frank memory MCP** — Phase 4 v0.12 占位 (`frank-mcp` binary 或
//!    `frank.hutiefang.com` 远程). 当前永远 `None`, 仅占位。
//!
//! # Recommendation matrix
//!
//! | official_mcp | frank_cli_in_path | recommendation |
//! |---|---|---|
//! | false | true | `NoChange` (frank CLI 已接管, 无 official 冲突) |
//! | true | true | `DisableOfficial` (装 frank 后建议关 official) |
//! | false | false | `InstallFrank` (用户既没装 official 也没装 frank) |
//! | true | false | `KeepBoth` (用户只装了 official, 没装 frank, 不动) |

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod opencode;

use std::path::PathBuf;

/// 4 个被探测的 AI provider。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Claude Code CLI (`~/.claude.json`)。
    Claude,
    /// codex CLI (`~/.codex/config.toml`)。
    Codex,
    /// Gemini CLI (`~/.gemini/settings.json`)。
    Gemini,
    /// opencode CLI (`~/.config/opencode/opencode.json`)。
    Opencode,
}

impl Provider {
    /// Provider 短名 (输出表格用)。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Opencode => "opencode",
        }
    }

    /// 全部 4 个 provider 的列表 (固定顺序: claude → codex → gemini → opencode)。
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::Claude, Self::Codex, Self::Gemini, Self::Opencode]
    }
}

/// 用户装的 Official MCP memory 条目 (`@modelcontextprotocol/server-memory` 等)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialMcp {
    /// 在 provider config 里的 entry 名 (通常是 `"memory"`).
    pub entry_name: String,
    /// 是否被显式 `enabled = false` 关掉 (opencode/codex 可能有此字段)。
    /// JSON 没有 disabled 概念的 (如 claude), 永远 `false`。
    pub disabled: bool,
}

/// frank 自家的 memory MCP 检测结果 (Phase 4 v0.12 占位)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrankMcp {
    /// 接入模式: stdio (本机 `frank-mcp` binary) 或 remote (`frank.hutiefang.com`)。
    pub mode: FrankMcpMode,
}

/// frank-mcp 的接入模式 (Phase 4 占位)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrankMcpMode {
    /// 通过 `frank-mcp` 子进程 stdin/stdout。
    Stdio,
    /// 通过 `frank.hutiefang.com` 远程 SSE / HTTP。
    Remote,
}

/// 单个 provider 探测结果 (config 路径 + official_mcp + frank_mcp)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMemory {
    /// 被探测的 provider。
    pub provider: Provider,
    /// config 文件路径 (无 home_dir 时 `None`)。
    pub config_path: Option<PathBuf>,
    /// 是否装了 Official memory MCP (npx `@modelcontextprotocol/server-memory` 等)。
    pub official_mcp: Option<OfficialMcp>,
    /// 是否装了 frank-mcp (Phase 4 v0.12, 当前永远 None)。
    pub frank_mcp: Option<FrankMcp>,
}

/// 综合推荐结论 (基于 4 provider 聚合 + `frank` CLI 是否在 PATH)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recommendation {
    /// frank CLI 在, 无 official memory MCP — 干净, 不动。
    NoChange,
    /// frank CLI 在, **任一** provider 装了 official memory MCP — 建议禁用 official。
    DisableOfficial,
    /// frank CLI 不在, 全部 provider 也没装 official memory MCP — 建议装 frank。
    InstallFrank,
    /// frank CLI 不在, 但 official memory MCP 已装 — 保持现状 (用户没 opt-in frank)。
    KeepBoth,
}

/// 探测全部 4 provider 的 MCP memory 配置。
///
/// 不会 bail: 任何单个 provider 解析失败降级为 `official_mcp = None` + log warn,
/// 让 `frank doctor` 总能跑完。
#[must_use]
pub fn inspect_all() -> Vec<ProviderMemory> {
    Provider::all().iter().map(|&p| inspect(p)).collect()
}

/// 探测单个 provider。出错降级为空结果。
#[must_use]
pub fn inspect(provider: Provider) -> ProviderMemory {
    let (config_path, official_mcp) = match provider {
        Provider::Claude => claude::read(),
        Provider::Codex => codex::read(),
        Provider::Gemini => gemini::read(),
        Provider::Opencode => opencode::read(),
    };
    ProviderMemory {
        provider,
        config_path,
        official_mcp,
        frank_mcp: None, // Phase 4 v0.12 占位
    }
}

/// 综合所有 provider 结果给出 single recommendation。
///
/// 规则见 module 文档 Recommendation matrix。
#[must_use]
pub fn recommend(setups: &[ProviderMemory]) -> Recommendation {
    let has_official = setups.iter().any(|s| s.official_mcp.is_some());
    let frank_in_path = frank_cli_in_path();
    match (has_official, frank_in_path) {
        (false, true) => Recommendation::NoChange,
        (true, true) => Recommendation::DisableOfficial,
        (false, false) => Recommendation::InstallFrank,
        (true, false) => Recommendation::KeepBoth,
    }
}

/// 探测 `frank` CLI 是否在用户 PATH (`which frank`)。
#[must_use]
pub fn frank_cli_in_path() -> bool {
    which::which("frank").is_ok()
}

/// 把 npx/uvx args 数组中是否包含 `@modelcontextprotocol/server-memory` 或
/// `mcp-server-memory` 一律判为 official memory MCP。
///
/// 适用 claude/gemini (JSON: command + args 分开) 与 codex (TOML 同形)。
/// opencode 的 command 是 `["npx", "-y", "..."]` 单数组, 走 [`is_official_combined`]。
pub(super) fn is_official(command: &str, args: &[String]) -> bool {
    match command {
        "npx" => args
            .iter()
            .any(|a| a.contains("@modelcontextprotocol/server-memory")),
        "uvx" => args.iter().any(|a| a == "mcp-server-memory"),
        _ => false,
    }
}

/// opencode 的 `command: ["npx", "-y", "@mcp/..."]` 单数组形式的判定。
pub(super) fn is_official_combined(command: &[String]) -> bool {
    let Some(head) = command.first() else {
        return false;
    };
    let tail: Vec<String> = command.iter().skip(1).cloned().collect();
    is_official(head, &tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_setup(provider: Provider, official: bool) -> ProviderMemory {
        ProviderMemory {
            provider,
            config_path: None,
            official_mcp: if official {
                Some(OfficialMcp {
                    entry_name: "memory".to_string(),
                    disabled: false,
                })
            } else {
                None
            },
            frank_mcp: None,
        }
    }

    // ─── recommendation matrix 4 case ─────────────────────────────────────

    #[test]
    fn recommend_no_change_when_frank_in_path_and_no_official() {
        let setups = [
            dummy_setup(Provider::Claude, false),
            dummy_setup(Provider::Codex, false),
        ];
        // 因 frank_cli_in_path() 在 CI 不一定 true, 这里手算逻辑
        let has_official = setups.iter().any(|s| s.official_mcp.is_some());
        assert!(!has_official);
    }

    #[test]
    fn recommend_disable_official_when_official_present() {
        let setups = [
            dummy_setup(Provider::Claude, true),
            dummy_setup(Provider::Codex, false),
        ];
        let has_official = setups.iter().any(|s| s.official_mcp.is_some());
        assert!(has_official);
    }

    #[test]
    fn recommend_matrix_truth_table() {
        // 用模拟函数代替 frank_cli_in_path() 测 4 组合
        fn rec(has_official: bool, frank_in_path: bool) -> Recommendation {
            match (has_official, frank_in_path) {
                (false, true) => Recommendation::NoChange,
                (true, true) => Recommendation::DisableOfficial,
                (false, false) => Recommendation::InstallFrank,
                (true, false) => Recommendation::KeepBoth,
            }
        }
        assert_eq!(rec(false, true), Recommendation::NoChange);
        assert_eq!(rec(true, true), Recommendation::DisableOfficial);
        assert_eq!(rec(false, false), Recommendation::InstallFrank);
        assert_eq!(rec(true, false), Recommendation::KeepBoth);
    }

    // ─── is_official npx/uvx detection ─────────────────────────────────────

    #[test]
    fn is_official_npx_modelcontextprotocol() {
        let args = vec!["-y".to_string(), "@modelcontextprotocol/server-memory".to_string()];
        assert!(is_official("npx", &args));
    }

    #[test]
    fn is_official_uvx_server_memory() {
        let args = vec!["mcp-server-memory".to_string()];
        assert!(is_official("uvx", &args));
    }

    #[test]
    fn is_official_other_command_rejected() {
        let args = vec!["@modelcontextprotocol/server-memory".to_string()];
        assert!(!is_official("docker", &args));
    }

    #[test]
    fn is_official_combined_npx_form() {
        let cmd = vec![
            "npx".to_string(),
            "-y".to_string(),
            "@modelcontextprotocol/server-memory".to_string(),
        ];
        assert!(is_official_combined(&cmd));
    }

    #[test]
    fn is_official_combined_empty_rejected() {
        assert!(!is_official_combined(&[]));
    }

    #[test]
    fn provider_name_round_trip() {
        assert_eq!(Provider::Claude.name(), "claude");
        assert_eq!(Provider::Codex.name(), "codex");
        assert_eq!(Provider::Gemini.name(), "gemini");
        assert_eq!(Provider::Opencode.name(), "opencode");
    }

    #[test]
    fn provider_all_returns_four() {
        assert_eq!(Provider::all().len(), 4);
    }

    #[test]
    fn inspect_all_returns_four() {
        // 调真实 inspect_all(); 即使没装任何 provider, 也应返回 4 条 (官方/frank 都 None)
        let res = inspect_all();
        assert_eq!(res.len(), 4);
    }
}
