//! `Provider` 枚举 — 各家 CLI 的凭据元数据 (env var 名 / official file 路径 / keyring service 名)。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 支持的 provider。新增 provider 在此加一个 variant + 各 metadata 函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic Claude (claude CLI / Claude Code)。
    Claude,
    /// OpenAI Codex (codex CLI)。
    Codex,
    /// Google Gemini (gemini CLI)。
    Gemini,
    /// SST opencode (opencode CLI)。
    Opencode,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Opencode => "opencode",
        };
        f.write_str(s)
    }
}

impl Provider {
    /// 所有支持的 provider, 用于 `frank login provider list` / `doctor` 遍历。
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::Claude, Self::Codex, Self::Gemini, Self::Opencode]
    }

    /// env var 名 (注入 child 用)。
    #[must_use]
    pub fn env_var_name(self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::Codex | Self::Opencode => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
        }
    }

    /// 各 CLI 的 setup-token 命令 (`frank login provider <name>` wrap)。
    ///
    /// 返回 `(binary, args)` 用于 `Command::new(binary).args(args)`。
    #[must_use]
    pub fn setup_command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::Claude => ("claude", &["setup-token"]),
            Self::Codex => ("codex", &["auth", "login"]),
            Self::Gemini => ("gemini", &["auth", "login"]),
            Self::Opencode => ("opencode", &["auth", "login"]),
        }
    }

    /// 各家 CLI 的 official credential file 候选路径 (按优先级排序)。
    ///
    /// 返回多个候选, 因为各 CLI 版本/平台不同位置。`official::lookup` 按序探。
    #[must_use]
    pub fn official_file_candidates(self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_default();
        match self {
            Self::Claude => vec![
                home.join(".claude").join(".credentials.json"),
                home.join(".config")
                    .join("anthropic")
                    .join("credentials.json"),
            ],
            Self::Codex => vec![
                home.join(".codex").join("credentials.toml"),
                home.join(".codex").join("auth.json"),
            ],
            Self::Gemini => {
                let mut v = vec![home.join(".gemini").join("credentials.json")];
                if let Some(cfg) = dirs::config_dir() {
                    v.push(cfg.join("gemini").join("credentials.json"));
                }
                v
            }
            Self::Opencode => {
                let mut v = vec![home.join(".opencode").join("auth.json")];
                if let Some(data) = dirs::data_local_dir() {
                    v.push(data.join("opencode").join("auth.json"));
                }
                v
            }
        }
    }

    /// macOS Keychain service name (用 `security find-generic-password -s <name>`)。
    ///
    /// 同时适用于 Windows Credential Manager target name 和 Linux Secret Service schema。
    #[must_use]
    pub fn keyring_service(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code-credentials",
            Self::Codex => "Codex CLI",
            Self::Gemini => "Gemini CLI",
            Self::Opencode => "opencode-cli",
        }
    }

    /// 解析字符串名 (e.g. CLI arg `frank login provider claude`)。
    ///
    /// # Errors
    ///
    /// 未知 provider 名返回 `Err`。
    pub fn parse_name(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "gemini" => Ok(Self::Gemini),
            "opencode" => Ok(Self::Opencode),
            other => Err(format!(
                "未知 provider: {other} (支持: claude/codex/gemini/opencode)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trip() {
        for p in Provider::all() {
            assert_eq!(Provider::parse_name(&p.to_string()).unwrap(), *p);
        }
    }

    #[test]
    fn env_var_names_correct() {
        assert_eq!(Provider::Claude.env_var_name(), "ANTHROPIC_API_KEY");
        assert_eq!(Provider::Codex.env_var_name(), "OPENAI_API_KEY");
        assert_eq!(Provider::Gemini.env_var_name(), "GEMINI_API_KEY");
        assert_eq!(Provider::Opencode.env_var_name(), "OPENAI_API_KEY");
    }

    #[test]
    fn setup_commands_present() {
        for p in Provider::all() {
            let (bin, args) = p.setup_command();
            assert!(!bin.is_empty());
            assert!(!args.is_empty());
        }
    }

    #[test]
    fn official_candidates_nonempty() {
        for p in Provider::all() {
            assert!(!p.official_file_candidates().is_empty(), "{p} 应有候选路径");
        }
    }

    #[test]
    fn parse_name_unknown_errs() {
        assert!(Provider::parse_name("foo").is_err());
    }
}
