//! Reciprocal Rank Fusion — Cormack/Clarke/Buettcher SIGIR 2009.
//!
//! 公式: `score(d) = Σᵢ wᵢ / (k + rankᵢ(d))`, 默认 `k=60`。
//!
//! 引用 (POSITION.md #4 / ADR-011):
//! - paper: <https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf>
//! - 业界 default 全是 60: OpenSearch / Qdrant / Elasticsearch / Milvus / LangChain
//! - 调参敏感性: k ∈ [10, 100] NDCG 波动 < 2%, 对 k 不敏感

use std::collections::HashMap;

use crate::memory::MemoryId;

/// 默认 k 常量, paper + 业界标配。
pub const DEFAULT_K: f64 = 60.0;

/// 融合 N 路排名列表 → 统一打分降序列表。
///
/// # 参数
/// - `ranked_lists`: 每路一个 `Vec<MemoryId>`, **已按相关性降序排** (第 0 个 = top-1)
/// - `k`: smoothing constant (默认 60)
/// - `weights`: 每路权重; `None` = 等权 (各路 1.0)。`Some` len 必须 == `ranked_lists.len()`
///
/// # 返回
/// `Vec<(MemoryId, f64)>` 按 RRF score 降序。score 越大越像。
///
/// # Panics
/// `weights` 长度跟 `ranked_lists` 不匹配时 panic (调用方编程错误)。
#[must_use]
pub fn fuse(
    ranked_lists: &[Vec<MemoryId>],
    k: f64,
    weights: Option<&[f64]>,
) -> Vec<(MemoryId, f64)> {
    let weights_vec: Vec<f64> =
        weights.map_or_else(|| vec![1.0; ranked_lists.len()], <[f64]>::to_vec);
    assert_eq!(
        weights_vec.len(),
        ranked_lists.len(),
        "weights len ({}) != ranked_lists len ({})",
        weights_vec.len(),
        ranked_lists.len()
    );

    let mut scores: HashMap<MemoryId, f64> = HashMap::new();
    for (list_idx, list) in ranked_lists.iter().enumerate() {
        let w = weights_vec[list_idx];
        for (rank, id) in list.iter().enumerate() {
            // rank 0-based; paper 1-based, +1 对齐
            #[allow(clippy::cast_precision_loss)]
            let contribution = w / (k + (rank + 1) as f64);
            *scores.entry(*id).or_insert(0.0) += contribution;
        }
    }

    let mut out: Vec<_> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn mid(seed: u8) -> MemoryId {
        // 固定 UUID 便于测试 (前 15 字节 0, 末字节 seed)
        let mut bytes = [0_u8; 16];
        bytes[15] = seed;
        MemoryId::from_uuid(Uuid::from_bytes(bytes))
    }

    /// 2 路对称: A/B 在两路位置互换 → score 应相等。
    #[test]
    fn rrf_two_path_symmetric() {
        let list_a = vec![mid(1), mid(2), mid(3)]; // A=1, B=2, C=3
        let list_b = vec![mid(2), mid(1), mid(3)]; // B=1, A=2, C=3
        let out = fuse(&[list_a, list_b], DEFAULT_K, None);
        assert_eq!(out.len(), 3);
        // A: 1/61 + 1/62 = 0.03252
        // B: 1/62 + 1/61 = 0.03252 (同)
        // C: 1/63 + 1/63 = 0.03175
        assert!((out[0].1 - out[1].1).abs() < 1e-9, "A 和 B 应等分");
        assert_eq!(out[2].0, mid(3));
    }

    /// 单路退化 = 原顺序。
    #[test]
    fn rrf_single_path_preserves_order() {
        let list = vec![mid(1), mid(2), mid(3), mid(4)];
        let out = fuse(std::slice::from_ref(&list), DEFAULT_K, None);
        let ids: Vec<MemoryId> = out.into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, list);
    }

    /// 空列表不 panic。
    #[test]
    fn rrf_empty_lists_no_panic() {
        let out = fuse(&[vec![], vec![]], DEFAULT_K, None);
        assert!(out.is_empty());
    }

    /// k=0 极端: top-1 拿 1.0, 跟 paper 限定一致。
    #[test]
    fn rrf_k_zero_top_rank_dominates() {
        let list_a = vec![mid(1), mid(2)];
        let list_b = vec![mid(2), mid(1)];
        let out = fuse(&[list_a, list_b], 0.0, None);
        // A: 1/1 + 1/2 = 1.5
        // B: 1/2 + 1/1 = 1.5 (同)
        assert_eq!(out.len(), 2);
        assert!((out[0].1 - 1.5).abs() < 1e-9);
        assert!((out[1].1 - 1.5).abs() < 1e-9);
    }

    /// 权重 10:1 放大第一路 → 第一路 top-1 应在融合 top.
    #[test]
    fn rrf_weight_amplifies_path() {
        let list_a = vec![mid(1), mid(2)]; // A=top
        let list_b = vec![mid(2), mid(1)]; // B=top
        let out = fuse(&[list_a, list_b], DEFAULT_K, Some(&[10.0, 1.0]));
        assert_eq!(out[0].0, mid(1), "权重 10x 让 list_a 主导, A 该排第一");
    }

    /// 单路 top 跟另一路 top 是同一个 doc → 该 doc 累计最高分。
    #[test]
    fn rrf_consensus_wins() {
        let list_a = vec![mid(1), mid(2), mid(3)];
        let list_b = vec![mid(1), mid(3), mid(2)]; // A 都是 top-1
        let out = fuse(&[list_a, list_b], DEFAULT_K, None);
        assert_eq!(out[0].0, mid(1), "两路都 top-1 的 A 应最高分");
    }

    /// 权重长度不匹配 → panic (编程错误).
    #[test]
    #[should_panic(expected = "weights len")]
    fn rrf_weights_mismatch_panics() {
        let list = vec![mid(1)];
        let _ = fuse(&[list.clone(), list], DEFAULT_K, Some(&[1.0])); // 2 路 1 权重
    }
}
