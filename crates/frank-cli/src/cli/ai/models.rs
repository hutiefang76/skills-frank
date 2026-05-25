//! `frank ai ask --list-models` — 列出 4 家 CLI 当前能用的模型。
//!
//! # 为什么要这个
//!
//! 用户在 `frank ai ask --to <provider> --model <name>` 调时, 经常不知道
//! 当前装的 CLI 到底支持哪些 model 名 (claude 有 sonnet/opus/haiku, 还有
//! `sonnet[1m]` 长上下文; codex 有 gpt-5.5/gpt-5.5-pro/o3 不少种). 一行
//! `--list-models` 直接列全, 省得开 4 家 docs 翻文档.
//!
//! # 数据来源 (3 路合并, 后来居上)
//!
//! 1. **frank 内置静态清单** — claude/codex/gemini 没有 `models` 子命令, 没法
//!    动态拿, 只能 frank 维护一份小白话清单 (跟新 CLI 版本一起更).
//! 2. **opencode 实时拉** — opencode 是唯一支持 `opencode models` 列模型的
//!    CLI, 直接 spawn 一次拿当前用户配的 (用户可能配 20+ 个 self-hosted model).
//! 3. **用户 `~/.frank/models.yaml` 覆盖** — 用户自己加额外 model 名 (例如
//!    frank 内置清单还没收的新模型), frank 跟内置合并显示.
//!
//! # 不装 CLI 怎么办
//!
//! 没装的 CLI 标 "⚠ 未装, 跑 `brew install <bin>`", 不阻塞其他 3 家正常列.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

/// 4 家 CLI 的内置 model 清单 (跟 CLI 版本一起手工维护).
///
/// 来源:
/// - claude: anthropic-quickstart 文档 + `claude --help` 实测
/// - codex: openai-codex 仓库 README + `~/.codex/config.toml` 配置示例
/// - gemini: `gemini --help` + Google AI docs
/// - opencode: opencode README (但 opencode 推荐用户跑 `opencode models` 实时拉)
const BUILTIN_MODELS: &[(&str, &[&str])] = &[
    (
        "claude",
        &["sonnet", "opus", "haiku", "sonnet[1m]", "opus[1m]"],
    ),
    ("codex", &["gpt-5.5", "gpt-5.5-pro", "o3", "gpt-5.4-mini"]),
    (
        "gemini",
        &["gemini-3.1-pro", "gemini-2.5-pro", "gemini-2.5-flash"],
    ),
    // opencode 内置只塞最常见 4 个; 真实模型靠 opencode_runtime_models() 实时拉.
    ("opencode", &["haiku", "qwen3.6", "gpt-4o-mini", "sonnet"]),
];

/// 4 家 CLI 的 binary 名 (跟 `invocation()` 保持一致).
const CLI_BINS: &[(&str, &str)] = &[
    ("claude", "claude"),
    ("codex", "codex"),
    ("opencode", "opencode"),
    ("gemini", "gemini"),
];

/// `~/.frank/models.yaml` 用户自定义模型表的 schema.
///
/// 每个 provider 对应一个 `Vec<String>` 列额外模型名 — 跟 frank 内置清单合并.
///
/// 文件不存在 = 返回空 map, 不报错.
#[derive(Debug, Default, Deserialize)]
pub struct UserModelsConfig {
    /// claude 额外模型名.
    #[serde(default)]
    pub claude: Vec<String>,
    /// codex 额外模型名.
    #[serde(default)]
    pub codex: Vec<String>,
    /// opencode 额外模型名.
    #[serde(default)]
    pub opencode: Vec<String>,
    /// gemini 额外模型名.
    #[serde(default)]
    pub gemini: Vec<String>,
}

/// 拿 `~/.frank/models.yaml` 路径 (跟其他 frank 文件一致, 走 dirs::home_dir).
fn user_models_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".frank").join("models.yaml"))
}

/// 读 `~/.frank/models.yaml`. 文件不存在 / 解析失败 → 返回 default (空).
///
/// **不报错** — 用户没配文件是正常情况, 不该打扰. 解析失败仅 tracing::warn.
pub fn load_user_overrides() -> UserModelsConfig {
    let Some(path) = user_models_path() else {
        return UserModelsConfig::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return UserModelsConfig::default();
    };
    match serde_yml::from_str::<UserModelsConfig>(&text) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!("~/.frank/models.yaml 解析失败 ({e}); 忽略用户自定义, 仅用内置清单");
            UserModelsConfig::default()
        }
    }
}

/// 跑 `opencode models` 拿当前装的 opencode 真实模型列表.
///
/// opencode 支持用户自配 N 个 model (self-hosted vLLM / OpenAI 代理 / qwen ...),
/// 内置清单可能漏一堆. 这个函数 spawn `opencode models` 子进程, parse stdout
/// 按行切, 返回所有非空行.
///
/// 失败 (binary 没装 / 超时 / 输出非预期) → 返回空 Vec, 调用方降级到内置清单.
fn opencode_runtime_models() -> Vec<String> {
    if which::which("opencode").is_err() {
        return Vec::new();
    }
    let mut cmd = Command::new("opencode");
    cmd.arg("models")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return Vec::new();
    };
    // 给 opencode 5s 拉清单, 超时直接 kill (避免阻塞 frank 主进程)
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(5) => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Vec::new(),
        }
    }
    let Ok(output) = child.wait_with_output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        // opencode models 输出可能有表头/分隔行 — 过滤掉明显非模型名 (含空格或 "Model" 字面量) 的
        .filter(|s| !s.contains(char::is_whitespace) && !s.eq_ignore_ascii_case("model"))
        .map(String::from)
        .collect()
}

/// 一个 provider 的模型清单 (内置 + opencode runtime + 用户覆盖, 已去重排序).
#[derive(Debug, Clone)]
pub struct ProviderModels {
    /// 模型名列表 (去重 + 保留输入顺序).
    pub models: Vec<String>,
    /// CLI binary 是否装了 (false 时打印用 "未装" 提示).
    pub installed: bool,
}

/// 算出每家 provider 的最终模型清单.
///
/// 流程:
/// 1. 从 BUILTIN_MODELS 拿内置.
/// 2. opencode 额外跑 `opencode models` 合并 runtime 模型.
/// 3. 跟 `~/.frank/models.yaml` 用户自定义合并 (用户的加在后面).
/// 4. 检查 binary 是否装 (which) 设置 installed flag.
/// 5. 去重保留顺序.
pub fn collect_all() -> HashMap<String, ProviderModels> {
    let user = load_user_overrides();
    let mut out: HashMap<String, ProviderModels> = HashMap::new();
    for (provider, builtin) in BUILTIN_MODELS {
        let bin = CLI_BINS
            .iter()
            .find(|(p, _)| p == provider)
            .map_or(*provider, |(_, b)| *b);
        let installed = which::which(bin).is_ok();

        // opencode 优先用 runtime 拉的 (用户自配的真实清单), runtime 拉空才走内置 fallback.
        // 其他 3 家没 `models` 子命令, 只能用内置.
        let mut models: Vec<String> = if *provider == "opencode" {
            let runtime = opencode_runtime_models();
            if runtime.is_empty() {
                builtin.iter().map(|s| (*s).to_string()).collect()
            } else {
                runtime
            }
        } else {
            builtin.iter().map(|s| (*s).to_string()).collect()
        };
        let extra = match *provider {
            "claude" => &user.claude,
            "codex" => &user.codex,
            "opencode" => &user.opencode,
            "gemini" => &user.gemini,
            _ => continue,
        };
        models.extend(extra.iter().cloned());

        // 去重保留顺序 (Vec<String> 用 IndexSet 太重, 手写循环即可).
        let mut seen = std::collections::HashSet::new();
        models.retain(|m| seen.insert(m.clone()));

        out.insert(
            (*provider).to_string(),
            ProviderModels { models, installed },
        );
    }
    out
}

/// 把所有 provider 的模型清单 print 到 stdout (一行一 provider).
///
/// 输出格式 (跟 PHASE-7-PLAN 对齐):
/// ```text
/// claude:    sonnet, opus, haiku, sonnet[1m], opus[1m]
/// codex:     gpt-5.5, gpt-5.5-pro, o3, gpt-5.4-mini
/// opencode:  haiku, qwen3.6, gpt-4o-mini, ... (从 opencode models 拉)
/// gemini:    gemini-3.1-pro, gemini-2.5-pro, gemini-2.5-flash
/// (未装的 CLI 标 "⚠ 未装")
/// ```
pub fn print_all() -> Result<()> {
    let all = collect_all();
    // 固定顺序 (claude → codex → opencode → gemini) 让输出可预测.
    let order = ["claude", "codex", "opencode", "gemini"];
    crate::log::ui::section("可用模型 (frank ai ask --to <provider> --model <name>)");
    for p in &order {
        if let Some(pm) = all.get(*p) {
            if pm.installed {
                println!("{:<10} {}", format!("{p}:"), pm.models.join(", "));
            } else {
                let bin = CLI_BINS
                    .iter()
                    .find(|(prov, _)| prov == p)
                    .map_or(*p, |(_, b)| *b);
                println!(
                    "{:<10} ⚠ 未装, 跑 `brew install {bin}` 装一下 (内置清单: {})",
                    format!("{p}:"),
                    pm.models.join(", ")
                );
            }
        }
    }
    // 提示用户怎么扩
    let cfg_path = user_models_path().map_or_else(
        || "~/.frank/models.yaml".to_string(),
        |p| p.display().to_string(),
    );
    crate::log::ui::info(&format!(
        "想加自己的模型? 编辑 {cfg_path} (YAML: 每个 provider 一个 list)"
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_models_have_4_providers() {
        // 防漏: 4 家全在
        let providers: Vec<&str> = BUILTIN_MODELS.iter().map(|(p, _)| *p).collect();
        assert!(providers.contains(&"claude"));
        assert!(providers.contains(&"codex"));
        assert!(providers.contains(&"opencode"));
        assert!(providers.contains(&"gemini"));
    }

    #[test]
    fn user_overrides_default_empty_when_no_file() {
        // ~/.frank/models.yaml 不存在 (CI 干净环境) → 全空 vec
        // 注意: 本地开发环境可能有该文件, 测试只断言数据结构对 (用 default 比较)
        let cfg = UserModelsConfig::default();
        assert!(cfg.claude.is_empty());
        assert!(cfg.codex.is_empty());
        assert!(cfg.opencode.is_empty());
        assert!(cfg.gemini.is_empty());
    }

    #[test]
    fn user_overrides_parses_yaml() {
        // 直接喂一段 YAML 给 serde_yml, 验证 schema 对
        let yaml = r"
claude:
  - claude-sonnet-4-7
codex:
  - gpt-5.5-experimental
opencode:
  - my-custom-model
";
        let cfg: UserModelsConfig = serde_yml::from_str(yaml).expect("parse YAML");
        assert_eq!(cfg.claude, vec!["claude-sonnet-4-7"]);
        assert_eq!(cfg.codex, vec!["gpt-5.5-experimental"]);
        assert_eq!(cfg.opencode, vec!["my-custom-model"]);
        assert!(cfg.gemini.is_empty()); // 没配 = 默认空
    }

    #[test]
    fn collect_all_returns_4_providers() {
        let all = collect_all();
        // 不管装没装 CLI, map 里 4 个 key 都该有
        assert!(all.contains_key("claude"));
        assert!(all.contains_key("codex"));
        assert!(all.contains_key("opencode"));
        assert!(all.contains_key("gemini"));
        // 内置清单非空
        assert!(!all["claude"].models.is_empty());
        assert!(!all["codex"].models.is_empty());
        assert!(!all["gemini"].models.is_empty());
    }
}
