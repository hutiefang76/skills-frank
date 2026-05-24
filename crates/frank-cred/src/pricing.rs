//! 模型定价表 — 编译期嵌入 `pricing-2026-05.json`, 运行时支持 `~/.frank/pricing.json` 覆盖。
//!
//! # 设计动机
//!
//! frank 跨进程调 claude / codex 等 CLI 后, 需要把 token 用量换算成 USD 显示给用户。
//! 价格随上游 API 涨跌, 因此:
//!
//! 1. **不写 Rust const** — 用 JSON, 升级只换 JSON, 不重编 binary
//! 2. **bundle 一份 baseline** — `include_str!` 直接打进 binary (无外部依赖)
//! 3. **用户可热替** — `~/.frank/pricing.json` 存在时优先用 (适合企业自配, 走代理打折)
//! 4. **未知模型** — 走 `unknown_model_policy`, 标 `Confidence::Low`
//!
//! # 单位
//!
//! JSON 内单价为 **USD / 1,000,000 tokens**。`compute_cost` 内部除以 `1_000_000.0`。

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 编译期嵌入的 baseline 定价表 JSON 字节。
const BUNDLED_JSON: &str = include_str!("../data/pricing-2026-05.json");

/// 一行定价规则: 输入 / 输出 / 缓存读 (可选) / 信心。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// 输入单价 (USD / 1M tokens)。
    pub input: f64,
    /// 输出单价 (USD / 1M tokens)。
    pub output: f64,
    /// Anthropic 缓存读单价 (USD / 1M tokens, 可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// OpenAI 缓存输入单价 (USD / 1M tokens, 可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<f64>,
    /// 信心: `"high"` / `"med"` / `"low"`。
    #[serde(default)]
    pub confidence: String,
}

/// 整张定价表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingTable {
    /// schema 版本号 (当前 = 1)。
    pub schema_version: u32,
    /// 表生效日期 (`YYYY-MM-DD`)。
    pub effective_date: String,
    /// 货币 (当前永远 `"USD"`)。
    pub currency: String,
    /// model 名 → 定价。
    pub models: HashMap<String, ModelPricing>,
    /// 找不到 model 时回退的价格 + `Confidence::Low`。
    pub unknown_model_policy: ModelPricing,
}

impl PricingTable {
    /// 加载 binary 内嵌的 baseline 表 (永不失败 — JSON 在编译期就过 parse)。
    ///
    /// # Panics
    ///
    /// `pricing-2026-05.json` 文件如果损坏会 panic, 这是编译期保证, 实际不会发生。
    #[must_use]
    pub fn load_bundled() -> Self {
        serde_json::from_str(BUNDLED_JSON).expect("bundled pricing JSON must parse at build time")
    }

    /// 优先用 `~/.frank/pricing.json`, 不存在则 fallback 到内嵌 baseline。
    ///
    /// 用户覆盖文件解析失败时 (JSON 坏) 也 fallback baseline, 仅记 `tracing::warn`,
    /// 避免坏 override 阻塞 CLI 主流程。
    #[must_use]
    pub fn load_with_override() -> Self {
        if let Some(path) = user_override_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => match serde_json::from_str::<Self>(&text) {
                        Ok(table) => return table,
                        Err(e) => {
                            tracing::warn!(
                                "{} parse failed, fallback bundled: {e}",
                                path.display()
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!("read {} failed, fallback bundled: {e}", path.display());
                    }
                }
            }
        }
        Self::load_bundled()
    }

    /// 按 model 名查定价。命中返回 `Some`, miss 返回 `None` (调用方决定要不要走 `unknown_model_policy`)。
    #[must_use]
    pub fn lookup(&self, model: &str) -> Option<&ModelPricing> {
        self.models.get(model)
    }

    /// 按 model 名查, miss 时返回 `unknown_model_policy` 与 `confidence = "low"`。
    ///
    /// 给 `CallReport` 走 "永远有 cost 估算" 的路径用。
    #[must_use]
    pub fn lookup_or_unknown(&self, model: &str) -> &ModelPricing {
        self.models.get(model).unwrap_or(&self.unknown_model_policy)
    }
}

/// 计算一次调用的 USD 成本。
///
/// 公式: `(input * rates.input + output * rates.output) / 1_000_000`。
///
/// 仅用 input/output 两项 (缓存读 / 缓存写不在这层算; 调用方需要时另算后相加)。
#[must_use]
pub fn compute_cost(input_tokens: u64, output_tokens: u64, rates: &ModelPricing) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let input_dollars = (input_tokens as f64) * rates.input / 1_000_000.0;
    #[allow(clippy::cast_precision_loss)]
    let output_dollars = (output_tokens as f64) * rates.output / 1_000_000.0;
    input_dollars + output_dollars
}

/// 用户覆盖文件路径: `~/.frank/pricing.json`。返 `None` 表示 home 解析失败。
fn user_override_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".frank").join("pricing.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_loads_without_panic() {
        let t = PricingTable::load_bundled();
        assert_eq!(t.schema_version, 1);
        assert_eq!(t.currency, "USD");
        assert!(!t.models.is_empty());
    }

    #[test]
    fn lookup_known_models() {
        let t = PricingTable::load_bundled();
        let opus = t.lookup("claude-opus-4-7").expect("opus must be present");
        assert!((opus.input - 5.00).abs() < f64::EPSILON);
        assert!((opus.output - 25.00).abs() < f64::EPSILON);
        assert_eq!(opus.confidence, "high");

        let gpt = t.lookup("gpt-5.5").expect("gpt-5.5 must be present");
        assert!((gpt.input - 5.00).abs() < f64::EPSILON);
        assert_eq!(gpt.confidence, "med");
    }

    #[test]
    fn lookup_miss_returns_none() {
        let t = PricingTable::load_bundled();
        assert!(t.lookup("totally-fake-model-xyz").is_none());
    }

    #[test]
    fn lookup_or_unknown_falls_back_to_policy() {
        let t = PricingTable::load_bundled();
        let unknown = t.lookup_or_unknown("totally-fake-model-xyz");
        assert!((unknown.input - 5.00).abs() < f64::EPSILON);
        assert!((unknown.output - 25.00).abs() < f64::EPSILON);
        assert_eq!(unknown.confidence, "low");
    }

    #[test]
    fn compute_cost_basic() {
        let rates = ModelPricing {
            input: 5.00,
            output: 25.00,
            cache_read: None,
            cached_input: None,
            confidence: "high".to_string(),
        };
        // 1M input tokens @ $5/M + 1M output @ $25/M = $30
        let cost = compute_cost(1_000_000, 1_000_000, &rates);
        assert!((cost - 30.00).abs() < 1e-9);
    }

    #[test]
    fn compute_cost_zero_tokens() {
        let rates = ModelPricing {
            input: 5.00,
            output: 25.00,
            cache_read: None,
            cached_input: None,
            confidence: "high".to_string(),
        };
        let cost = compute_cost(0, 0, &rates);
        assert!(cost.abs() < f64::EPSILON);
    }

    #[test]
    fn compute_cost_small_input() {
        let rates = ModelPricing {
            input: 5.00,
            output: 25.00,
            cache_read: None,
            cached_input: None,
            confidence: "high".to_string(),
        };
        // 100 input @ $5/M + 50 output @ $25/M
        // = 100 * 5e-6 + 50 * 25e-6 = 5e-4 + 1.25e-3 = 1.75e-3
        let cost = compute_cost(100, 50, &rates);
        assert!((cost - 0.00175).abs() < 1e-9);
    }

    #[test]
    fn unknown_policy_is_low_confidence() {
        let t = PricingTable::load_bundled();
        assert_eq!(t.unknown_model_policy.confidence, "low");
    }

    #[test]
    fn load_with_override_falls_back_when_no_override() {
        // 测试环境没有 ~/.frank/pricing.json, 退到 bundled.
        // (不真去碰 user 的 ~/.frank, 这里只验证函数能返回有效 table)
        let t = PricingTable::load_with_override();
        assert_eq!(t.schema_version, 1);
        assert!(t.lookup("claude-sonnet-4-6").is_some());
    }
}
