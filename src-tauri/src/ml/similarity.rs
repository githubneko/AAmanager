//! 余弦相似度与 Top-K 检索。
//!
//! 纯 Rust 实现，无外部依赖。对应策划书 4.6 / 4.7 与 6.5 节。
//! 向量规模在 5000 条以内时全量计算即可满足「< 500 毫秒」的目标；
//! 更大规模再按策划书 4.7 引入 sqlite-vec / HNSW。

/// 计算两个向量的余弦相似度，范围 -1.0 - 1.0。
///
/// 任一向量为空或模长为 0 时返回 0.0（而不是 NaN）。
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut norm_a = 0f32;
    let mut norm_b = 0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a <= f32::EPSILON || norm_b <= f32::EPSILON {
        return 0.0;
    }
    let v = dot / (norm_a.sqrt() * norm_b.sqrt());
    if v.is_finite() {
        v.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// 把向量归一化为单位长度（原地）。
pub fn normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// 在一组候选向量中找出与 `query` 最相似的前 `k` 个。
///
/// 返回 `(id, 相似度)`，按相似度降序。`threshold` 以下的会被过滤。
pub fn top_k(
    query: &[f32],
    candidates: &[(i64, Vec<f32>)],
    k: usize,
    threshold: f32,
) -> Vec<(i64, f32)> {
    let mut scored: Vec<(i64, f32)> = candidates
        .iter()
        .map(|(id, v)| (*id, cosine_similarity(query, v)))
        .filter(|(_, s)| *s >= threshold)
        .collect();

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            // 相似度相同时按 id 升序，保证结果稳定
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(k);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_are_fully_similar() {
        let a = vec![1.0f32, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_are_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_are_minus_one() {
        let a = vec![1.0f32, 1.0];
        let b = vec![-1.0f32, -1.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn scale_invariance() {
        let a = vec![1.0f32, 2.0, 3.0];
        let b = vec![10.0f32, 20.0, 30.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zero_and_mismatched_vectors_do_not_panic() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn normalize_produces_unit_length() {
        let mut v = vec![3.0f32, 4.0];
        normalize(&mut v);
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((len - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_leaves_zero_vector_alone() {
        let mut v = vec![0.0f32, 0.0];
        normalize(&mut v);
        assert_eq!(v, vec![0.0, 0.0]);
    }

    #[test]
    fn top_k_sorts_and_truncates() {
        let query = vec![1.0f32, 0.0];
        let candidates = vec![
            (1i64, vec![1.0f32, 0.0]),  // 1.0
            (2i64, vec![0.7f32, 0.7]),  // ~0.707
            (3i64, vec![0.0f32, 1.0]),  // 0.0
            (4i64, vec![-1.0f32, 0.0]), // -1.0
        ];
        let r = top_k(&query, &candidates, 2, 0.5);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, 1);
        assert_eq!(r[1].0, 2);
        assert!(r[0].1 >= r[1].1);
    }

    #[test]
    fn top_k_applies_threshold() {
        let query = vec![1.0f32, 0.0];
        let candidates = vec![(1i64, vec![0.0f32, 1.0]), (2i64, vec![1.0f32, 0.0])];
        let r = top_k(&query, &candidates, 10, 0.85);
        assert_eq!(r.len(), 1, "低相似度应被阈值过滤");
        assert_eq!(r[0].0, 2);
    }

    #[test]
    fn top_k_is_stable_for_equal_scores() {
        let query = vec![1.0f32, 0.0];
        let candidates = vec![
            (5i64, vec![1.0f32, 0.0]),
            (3i64, vec![1.0f32, 0.0]),
            (9i64, vec![1.0f32, 0.0]),
        ];
        let r = top_k(&query, &candidates, 3, 0.0);
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![3, 5, 9]);
    }

    #[test]
    fn top_k_on_empty_candidates() {
        assert!(top_k(&[1.0, 0.0], &[], 5, 0.0).is_empty());
    }

    #[test]
    fn similarity_inheritance_threshold_behaves_as_documented() {
        // 策划书 4.6：相似度 > 0.85 才自动继承
        let new_audio = vec![1.0f32, 0.0, 0.0];
        let labeled = vec![
            (1i64, vec![0.99f32, 0.05, 0.0]), // 很相似 -> 继承
            (2i64, vec![0.5f32, 0.8, 0.2]),   // 一般 -> 不继承
        ];
        let inherited = top_k(&new_audio, &labeled, 3, 0.85);
        assert_eq!(inherited.len(), 1);
        assert_eq!(inherited[0].0, 1);
        assert!(inherited[0].1 > 0.85);
    }

    #[test]
    fn handles_512_dim_vectors() {
        // CLAP 输出维度
        let a: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let b: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin() * 2.0).collect();
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-4);
    }
}
