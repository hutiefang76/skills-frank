//! v0.15: models.dev 动态模型注册表 — 解决"模型列表写死"问题 (功能5 点3).
//!
//! # 为什么
//!
//! v0.10.8 的模型列表 = CLI 配置文件 + env + **写死的 `BUILTIN_ALIASES`** (sonnet/opus/
//! haiku, gpt-5.5 ...). 用户原话: "模型列表不是写死, 是动态加载, 做不到就每次启动 / 过几
//! 小时加载下"。
//!
//! # 方案
//!
//! 拉 [models.dev](https://models.dev/api.json) — opencode 用的权威跨厂模型库, **无需 key**,
//! 一份 JSON 列全部 provider 的当前模型 (含 release_date)。本地 12h disk cache, 过期才重拉,
//! 拉不到 / 无网 → 用 stale cache → 都没有才落回 `BUILTIN_ALIASES`。
//!
//! # 缓存
//!
//! `~/.frank/cache/models/models-dev.json` (整份 api.json). mtime <12h 直读 (秒级);
//! 过期重拉重写; 网络失败 fallback stale cache。一次 `frank refresh-skills` 只拉一次
//! (4 个 provider 共享同一份 cache, 第一个触发拉取, 后 3 个命中热 cache)。
//!
//! # provider 映射
//!
//! frank provider → models.dev 顶层 key:
//! - `claude`  → `anthropic`
//! - `codex`   → `openai`
//! - `gemini`  → `google`
//! - `opencode` → 无 (用户完全自配, 同 `BUILTIN_ALIASES` 也不兜底 opencode)

use std::path::PathBuf;
use std::time::Duration;

use super::{ModelEntry, ModelSource};

/// models.dev api.json 地址.
const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// disk cache TTL — 12 小时 (用户说"过几小时加载下").
const CACHE_TTL_SECS: u64 = 12 * 3600;

/// 每 provider 从 registry 最多取几个 (按 release_date 倒序, 新的在前).
/// refresh-skills 还会再 cap 到 5, 这里给宽点让 `--list-models` 多看几个当前模型.
const MAX_PER_PROVIDER: usize = 8;

/// 拉某 provider 的当前模型 (models.dev 动态源). 失败返回空 Vec (调用方落兜底).
#[must_use]
pub fn read_models(provider: &str) -> Vec<ModelEntry> {
    let Some(registry_key) = frank_provider_to_registry(provider) else {
        return Vec::new(); // opencode / 未知 → 不走 registry
    };
    let Some(root) = cached_api_json() else {
        return Vec::new();
    };
    let Some(models) = root
        .get(registry_key)
        .and_then(|p| p.get("models"))
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };

    // 收 (model_id, release_date) — 按 release_date 倒序 (新的在前), 取前 MAX_PER_PROVIDER.
    let mut pairs: Vec<(String, String)> = models
        .iter()
        .filter_map(|(id, cfg)| {
            // 跳过明显废弃 / legacy 的 (有 release_date 才算"当前模型")
            let date = cfg
                .get("release_date")
                .and_then(serde_json::Value::as_str)
                .or_else(|| cfg.get("last_updated").and_then(serde_json::Value::as_str))
                .unwrap_or("")
                .to_string();
            Some((id.clone(), date))
        })
        .collect();
    // release_date 倒序 (字符串 YYYY-MM-DD 字典序即时间序); 空 date 沉底.
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    pairs.truncate(MAX_PER_PROVIDER);

    pairs
        .into_iter()
        .map(|(id, _)| ModelEntry {
            name: id,
            source: ModelSource::Registry,
        })
        .collect()
}

/// frank provider 名 → models.dev 顶层 provider key.
fn frank_provider_to_registry(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some("anthropic"),
        "codex" => Some("openai"),
        "gemini" => Some("google"),
        // opencode: 用户自配任意后端, models.dev 没"opencode" 这个 provider 概念 → 不兜底
        _ => None,
    }
}

/// cache 文件路径 `~/.frank/cache/models/models-dev.json`.
fn cache_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".frank")
            .join("cache")
            .join("models")
            .join("models-dev.json"),
    )
}

/// 拿 models.dev api.json (整份). 12h cache, 过期重拉, 失败 fallback stale.
fn cached_api_json() -> Option<serde_json::Value> {
    let path = cache_path()?;

    // 1. 12h 内热 cache 直读
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < CACHE_TTL_SECS {
                    if let Ok(s) = std::fs::read_to_string(&path) {
                        if let Ok(v) = serde_json::from_str(&s) {
                            tracing::debug!(age_sec = elapsed.as_secs(), "models.dev cache hit");
                            return Some(v);
                        }
                    }
                }
            }
        }
    }

    // 2. 过期 / 无 cache → 拉 (短超时, 不拖累 refresh)
    match fetch_models_dev() {
        Ok(body) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &body);
            serde_json::from_str(&body).ok()
        }
        Err(e) => {
            // 3. 拉失败 → fallback stale cache (offline 友好)
            tracing::warn!(error = %e, "models.dev fetch failed, trying stale cache");
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        }
    }
}

/// blocking GET models.dev. 8s 超时 (refresh 不该卡太久).
fn fetch_models_dev() -> anyhow::Result<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("frank-cli/models-registry")
        .build()?;
    let resp = client.get(MODELS_DEV_URL).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("models.dev returned {}", resp.status());
    }
    Ok(resp.text()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_mapping() {
        assert_eq!(frank_provider_to_registry("claude"), Some("anthropic"));
        assert_eq!(frank_provider_to_registry("codex"), Some("openai"));
        assert_eq!(frank_provider_to_registry("gemini"), Some("google"));
        assert_eq!(frank_provider_to_registry("opencode"), None);
        assert_eq!(frank_provider_to_registry("nope"), None);
    }

    #[test]
    fn parse_models_dev_shape_sorts_by_date_desc() {
        // 模拟 models.dev 结构, 验证按 release_date 倒序取前 N
        let root: serde_json::Value = serde_json::json!({
            "anthropic": {
                "id": "anthropic",
                "models": {
                    "claude-old": {"id": "claude-old", "release_date": "2024-01-01"},
                    "claude-new": {"id": "claude-new", "release_date": "2025-06-01"},
                    "claude-mid": {"id": "claude-mid", "release_date": "2025-01-01"},
                }
            }
        });
        let models = root
            .get("anthropic")
            .and_then(|p| p.get("models"))
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let mut pairs: Vec<(String, String)> = models
            .iter()
            .map(|(id, cfg)| {
                (
                    id.clone(),
                    cfg.get("release_date")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));
        assert_eq!(pairs[0].0, "claude-new");
        assert_eq!(pairs[1].0, "claude-mid");
        assert_eq!(pairs[2].0, "claude-old");
    }

    #[test]
    fn read_models_opencode_empty() {
        // opencode 不走 registry, 直接空 (不碰网络)
        assert!(read_models("opencode").is_empty());
    }
}
