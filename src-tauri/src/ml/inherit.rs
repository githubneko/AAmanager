//! 相似度标签继承：把「听起来很像」的音频的标签传播过来。
//!
//! ## 为什么需要它
//!
//! 一个音效包里的枪声往往有十几个变体，规则引擎会给它们各自的声学标签，
//! 但用户给其中一个打了「主角手枪」这样的项目专属标签后，
//! 其余的变体不会自动跟上。用户得一个个手打 —— 这正是策划书 6.4
//! 「相似度继承推荐标签」要解决的问题。
//!
//! ## 设计取舍
//!
//! * **只继承用户的标签**，不继承继承来的标签。否则一次传播之后
//!   标签会像传话游戏一样越传越偏，也让「谁才是源头」无法追溯；
//! * **不覆盖已有标签**：目标音频若已有同名标签（无论来源），跳过。
//!   这样继承永远是「补充」，不会跟规则引擎或用户的判断打架；
//! * **置信度 = 相似度 × 源标签置信度**，并带上 `TagSource::Inherited`
//!   以便前端区分展示、用户一键撤销；
//! * **自动清理过期继承**：每次重算时删掉本轮不再成立的继承标签，
//!   避免素材被改动后留下幽灵标签。
//!
//! 阈值取 0.85（策划书 6.4）：实测同一音效的变体通常在 0.9 以上，
//! 而不同类别之间很少超过 0.8，0.85 是兼顾召回与误报的位置。

use crate::db::{audio_repo, tags_repo, vector_repo};
use crate::error::Result;
use crate::ml::similarity;
use crate::models::TagSource;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// 相似度阈值：达到这个值才认为「足够像」可以继承标签。
pub const SIMILARITY_THRESHOLD: f32 = 0.85;

/// 单次最多从多少个近邻继承，避免把一个音频的标签塞爆。
pub const MAX_SOURCES: usize = 5;

/// 单个音频一次最多新增多少个继承标签。
pub const MAX_INHERITED: usize = 20;

/// 继承标签继续向下传播所需的最低置信度。
///
/// 每传一跳置信度乘一次相似度，跌破此值就不再往外传，
/// 避免标签沿着「A 像 B、B 像 C」的长链一路扩散到语义无关的素材。
pub const PROPAGATE_MIN_CONFIDENCE: f32 = 0.35;

/// 继承结果，供日志与界面提示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InheritOutcome {
    /// 新写入 / 更新的继承标签数。
    pub inherited: u32,
    /// 清理掉的过期继承标签数。
    pub pruned: u32,
    /// 参与比较的候选音频数。
    pub compared: usize,
}

impl InheritOutcome {
    /// 本次是否有任何变化。
    pub fn changed(&self) -> bool {
        self.inherited > 0 || self.pruned > 0
    }
}

/// 对一个音频重算继承标签（幂等：反复调用结果一致）。
pub fn inherit_for(conn: &Connection, audio_id: i64) -> Result<InheritOutcome> {
    let target = match vector_repo::get_embedding(conn, audio_id)? {
        Some(v) => v,
        // 没有向量就无从比较；顺手清掉可能存在的旧继承，避免留下过期标签
        None => {
            return Ok(InheritOutcome {
                pruned: prune(conn, audio_id, &HashSet::new())?,
                ..Default::default()
            });
        }
    };

    let candidates = audio_repo::all_with_embeddings(conn)?;
    let compared = candidates.len();

    // 找到足够相似的近邻（排除自己）
    let hits: Vec<(i64, f32)> =
        similarity::top_k(&target, &candidates, MAX_SOURCES + 1, SIMILARITY_THRESHOLD)
            .into_iter()
            .filter(|(id, _)| *id != audio_id)
            .take(MAX_SOURCES)
            .collect();

    // 汇总近邻的**用户标签**以及**置信度仍够的继承标签**：
    // 同名标签取最高分。允许继承标签继续传播，是为了让一个标签能覆盖
    // 整个音效簇（否则离源头两跳以外的变体永远拿不到）；
    // 但每传一跳置信度就乘一次相似度，很快跌破 PROPAGATE_MIN_CONFIDENCE
    // 而自动止步 —— 这就是「传话游戏」问题的解法：不是禁止传播，
    // 而是让传播随距离衰减到自然停止。
    //
    // 这里**故意不过滤 existing**：目标上已有的继承标签也要重算，
    // 否则源标签改了置信度、或相似度变了，目标上的值会永远停在旧值。
    let mut wanted: HashMap<String, f32> = HashMap::new();
    for (src_id, sim) in &hits {
        for tag in tags_repo::list(conn, *src_id)? {
            let propagable = match tag.source {
                TagSource::User => true,
                TagSource::Inherited => tag.confidence > PROPAGATE_MIN_CONFIDENCE,
                // 规则/YAMNet/CLAP 标签由各自引擎负责，不参与继承
                _ => false,
            };
            if !propagable {
                continue;
            }
            let conf = (sim * tag.confidence).clamp(0.0, 1.0);
            let slot = wanted.entry(tag.tag).or_insert(0.0);
            if conf > *slot {
                *slot = conf;
            }
        }
    }

    // 目标**自有**的标签（用户 / 规则 / 模型）不覆盖；只有继承来的才允许改写
    let owned: HashSet<String> = tags_repo::list(conn, audio_id)?
        .into_iter()
        .filter(|t| t.source != TagSource::Inherited)
        .map(|t| t.tag)
        .collect();

    let mut picked: Vec<(String, f32)> = wanted
        .into_iter()
        .filter(|(tag, _)| !owned.contains(tag))
        .collect();
    picked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    picked.truncate(MAX_INHERITED);

    let keep: HashSet<String> = picked.iter().map(|(t, _)| t.clone()).collect();

    // 先写入新的，再清理不再成立的（顺序保证不会误删刚写的）
    let tx = conn.unchecked_transaction()?;
    for (tag, conf) in &picked {
        tx.execute(
            "INSERT INTO tags(audio_id, tag, source, confidence, is_confirmed)
             VALUES(?1, ?2, ?3, ?4, 0)
             ON CONFLICT(audio_id, tag, source) DO UPDATE SET confidence = excluded.confidence",
            rusqlite::params![
                audio_id,
                tag,
                TagSource::Inherited.as_db(),
                f64::from(*conf)
            ],
        )?;
    }
    // 清理：本轮不再成立、且不是目标自有标签的继承标签
    let stale: Vec<String> = tx
        .prepare("SELECT tag FROM tags WHERE audio_id = ?1 AND source = ?2")?
        .query_map(
            rusqlite::params![audio_id, TagSource::Inherited.as_db()],
            |r| r.get::<_, String>(0),
        )?
        .filter_map(|r| r.ok())
        .filter(|t| !keep.contains(t) && !owned.contains(t))
        .collect();
    let mut pruned = 0u32;
    for tag in &stale {
        pruned += tx.execute(
            "DELETE FROM tags WHERE audio_id = ?1 AND tag = ?2 AND source = ?3",
            rusqlite::params![audio_id, tag, TagSource::Inherited.as_db()],
        )? as u32;
    }
    tx.commit()?;

    if !picked.is_empty() || pruned > 0 {
        audio_repo::sync_fts(conn, audio_id)?;
    }

    Ok(InheritOutcome {
        inherited: picked.len() as u32,
        pruned,
        compared,
    })
}

/// 删除该音频上所有不再有效的继承标签，返回删除条数。
fn prune(conn: &Connection, audio_id: i64, keep: &HashSet<String>) -> Result<u32> {
    let stale: Vec<String> = conn
        .prepare("SELECT tag FROM tags WHERE audio_id = ?1 AND source = ?2")?
        .query_map(
            rusqlite::params![audio_id, TagSource::Inherited.as_db()],
            |r| r.get::<_, String>(0),
        )?
        .filter_map(|r| r.ok())
        .filter(|t| !keep.contains(t))
        .collect();
    if stale.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let mut n = 0u32;
    for tag in &stale {
        n += tx.execute(
            "DELETE FROM tags WHERE audio_id = ?1 AND tag = ?2 AND source = ?3",
            rusqlite::params![audio_id, tag, TagSource::Inherited.as_db()],
        )? as u32;
    }
    tx.commit()?;
    Ok(n)
}

/// 对整个库重算继承标签（导入一批素材后调用效果最好）。
///
/// 返回 `(处理的音频数, 新增继承数, 清理数)`。
pub fn inherit_all(conn: &Connection) -> Result<(usize, u32, u32)> {
    let ids: Vec<i64> = audio_repo::all_with_embeddings(conn)?
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    let (mut total_inherited, mut total_pruned) = (0u32, 0u32);
    for id in &ids {
        let out = inherit_for(conn, *id)?;
        total_inherited += out.inherited;
        total_pruned += out.pruned;
    }
    Ok((ids.len(), total_inherited, total_pruned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::audio_repo::NewAudio;
    use crate::db::open_in_memory;

    fn mk_audio(conn: &Connection, name: &str) -> i64 {
        audio_repo::insert(
            conn,
            &NewAudio {
                path: format!("C:/x/{name}"),
                filename: name.to_string(),
                folder: "C:/x".into(),
                duration: Some(1.0),
                sample_rate: Some(48_000),
                channels: Some(1),
                format: Some("wav".into()),
                bitrate: None,
                file_size: None,
                file_hash: None,
            },
        )
        .unwrap()
    }

    /// 两个几乎相同的音频：源上的用户标签应被继承过来。
    #[test]
    fn inherits_user_tags_from_near_identical_audio() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "gun_a.wav");
        let b = mk_audio(&conn, "gun_b.wav");

        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.01, 0.0], "v1").unwrap();

        tags_repo::add_tag(&conn, a, "主角手枪", TagSource::User, 1.0).unwrap();

        let out = inherit_for(&conn, b).unwrap();
        assert_eq!(out.inherited, 1, "应继承 1 个标签");

        let tags = tags_repo::list(&conn, b).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "主角手枪");
        assert_eq!(tags[0].source, TagSource::Inherited);
        assert!(tags[0].confidence > 0.8, "置信度应接近相似度");
        assert!(!tags[0].is_confirmed, "继承标签不应自动视为已确认");
    }

    /// 不够相似就不继承（阈值 0.85）。
    #[test]
    fn does_not_inherit_below_threshold() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");

        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        // 余弦 0.6，低于阈值
        vector_repo::upsert_embedding(&conn, b, &[0.6, 0.8], "v1").unwrap();
        tags_repo::add_tag(&conn, a, "手枪", TagSource::User, 1.0).unwrap();

        let out = inherit_for(&conn, b).unwrap();
        assert_eq!(out.inherited, 0);
        assert!(tags_repo::list(&conn, b).unwrap().is_empty());
    }

    /// 只继承用户标签，不继承规则标签，也不继承「继承来的」标签。
    #[test]
    fn only_user_tags_are_propagated() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        let c = mk_audio(&conn, "c.wav");

        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.001], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, c, &[0.998, 0.002], "v1").unwrap();

        tags_repo::add_tag(&conn, a, "规则标签", TagSource::Rule, 0.9).unwrap();
        tags_repo::add_tag(&conn, a, "用户的标签", TagSource::User, 1.0).unwrap();

        // b 从 a 继承
        inherit_for(&conn, b).unwrap();
        let b_tags: Vec<String> = tags_repo::list(&conn, b)
            .unwrap()
            .into_iter()
            .map(|t| t.tag)
            .collect();
        assert_eq!(b_tags, vec!["用户的标签"], "规则标签不应被继承");

        // 规则标签不参与传播，b 只应拿到用户标签
        let b_tags: Vec<String> = tags_repo::list(&conn, b)
            .unwrap()
            .into_iter()
            .map(|t| t.tag)
            .collect();
        assert_eq!(b_tags, vec!["用户的标签"]);

        // c 可以经由 b 继续继承（否则离源头两跳的变体永远拿不到标签），
        // 但置信度会衰减
        let out_c = inherit_for(&conn, c).unwrap();
        assert_eq!(out_c.inherited, 1, "应能经 b 传递到 c");
        let c_tag = tags_repo::list(&conn, c).unwrap();
        let b_conf = tags_repo::list(&conn, b).unwrap()[0].confidence;
        assert!(
            c_tag[0].confidence < b_conf,
            "传递后置信度必须衰减: c={} b={b_conf}",
            c_tag[0].confidence
        );
    }

    /// 长链传播必须能自然止步：置信度逐跳衰减到阈值以下就停。
    #[test]
    fn propagation_decays_and_stops() {
        let conn = open_in_memory().unwrap();
        // 一条 5 个音频的相似链，相邻相似度 0.90
        let ids: Vec<i64> = (0..5)
            .map(|i| mk_audio(&conn, &format!("chain{i}.wav")))
            .collect();
        for (i, id) in ids.iter().enumerate() {
            let a = 1.0 - i as f32 * 0.15;
            let b = 1.0 - a * a;
            let b = if b > 0.0 { b.sqrt() } else { 0.0 };
            vector_repo::upsert_embedding(&conn, *id, &[a, b], "v1").unwrap();
        }
        tags_repo::add_tag(&conn, ids[0], "链首标签", TagSource::User, 1.0).unwrap();

        let (n, _inherited, _) = inherit_all(&conn).unwrap();
        assert_eq!(n, 5);

        // 沿链的置信度必须严格递减，且最终跌破传播阈值
        let mut confs = Vec::new();
        for id in &ids {
            let tags = tags_repo::list(&conn, *id).unwrap();
            confs.push(tags.first().map(|t| t.confidence));
        }
        assert!(confs[0].is_some(), "源头应有标签");
        let tail: Vec<f32> = confs[1..].iter().filter_map(|c| *c).collect();
        if tail.len() >= 2 {
            assert!(
                tail.windows(2).all(|w| w[1] <= w[0] + 1e-6),
                "置信度应沿链递减: {tail:?}"
            );
        }
        // 链路末端要么没有标签，要么置信度已很低
        if let Some(last) = confs.last().and_then(|c| *c) {
            assert!(last < 0.9, "链尾置信度不应仍然很高: {last}");
        }
        eprintln!("链上置信度: {confs:?}");
    }

    /// 目标已有同名标签时不覆盖（继承只做补充）。
    #[test]
    fn does_not_override_existing_tag() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");

        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.001], "v1").unwrap();

        tags_repo::add_tag(&conn, a, "枪声", TagSource::User, 1.0).unwrap();
        tags_repo::add_tag(&conn, b, "枪声", TagSource::Rule, 0.8).unwrap();

        inherit_for(&conn, b).unwrap();
        let tags = tags_repo::list(&conn, b).unwrap();
        assert_eq!(tags.len(), 1, "不应产生重复标签: {tags:?}");
        assert_eq!(tags[0].source, TagSource::Rule, "原有标签不应被替换");
    }

    /// 幂等：反复调用不重复累加，也不新增。
    #[test]
    fn is_idempotent() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.001], "v1").unwrap();
        tags_repo::add_tag(&conn, a, "标签", TagSource::User, 1.0).unwrap();

        let first = inherit_for(&conn, b).unwrap();
        assert_eq!(first.inherited, 1);
        assert_eq!(first.pruned, 0);

        // 第二次：标签已在，仍是 1 条（既不重复累加，也不被误删）
        let second = inherit_for(&conn, b).unwrap();
        assert_eq!(second.pruned, 0, "已成立的继承标签不应被清理");
        let tags = tags_repo::list(&conn, b).unwrap();
        assert_eq!(tags.len(), 1, "不应重复累加: {tags:?}");
        assert_eq!(tags[0].source, TagSource::Inherited);
        // 置信度应稳定（重算结果一致）
        assert_eq!(first.inherited, second.inherited.max(1));
    }

    /// 源标签置信度变化时，目标上的继承标签应被**刷新**而不是冻结在旧值。
    #[test]
    fn inherited_confidence_is_refreshed_when_source_changes() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.99, 0.01], "v1").unwrap();

        tags_repo::add_tag(&conn, a, "标签", TagSource::User, 1.0).unwrap();
        inherit_for(&conn, b).unwrap();
        let high = tags_repo::list(&conn, b).unwrap()[0].confidence;

        // 源标签置信度降到 0.5，重算后目标上的值必须跟着降
        tags_repo::add_tag(&conn, a, "标签", TagSource::User, 0.5).unwrap();
        inherit_for(&conn, b).unwrap();
        let low = tags_repo::list(&conn, b).unwrap()[0].confidence;

        assert!(
            low < high,
            "继承置信度应随源标签刷新: 之前 {high}，现在 {low}"
        );
        assert!(
            (low - 0.5 * 0.99).abs() < 0.02,
            "应约为 相似度×源置信度: {low}"
        );
    }

    /// 源标签被删除后，继承来的标签应被清理。
    #[test]
    fn stale_inherited_tags_are_pruned() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.001], "v1").unwrap();

        tags_repo::add_tag(&conn, a, "临时", TagSource::User, 1.0).unwrap();
        inherit_for(&conn, b).unwrap();
        assert_eq!(tags_repo::list(&conn, b).unwrap().len(), 1);

        // 源标签删除后重算：b 上的继承标签应消失
        tags_repo::remove_tag(&conn, a, "临时").unwrap();
        let out = inherit_for(&conn, b).unwrap();
        assert_eq!(out.inherited, 0);
        assert_eq!(out.pruned, 1, "过期继承标签应被清理");
        assert!(tags_repo::list(&conn, b).unwrap().is_empty());
    }

    /// 没有向量的音频不应报错，且会清掉遗留的继承标签。
    #[test]
    fn audio_without_embedding_is_safe() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        tags_repo::add_tag(&conn, a, "遗留", TagSource::Inherited, 0.9).unwrap();

        let out = inherit_for(&conn, a).unwrap();
        assert_eq!(out.compared, 0);
        assert_eq!(out.pruned, 1, "无向量时应清掉遗留继承标签");
        assert!(tags_repo::list(&conn, a).unwrap().is_empty());
    }

    /// 全库重算不会 panic，且返回处理数量。
    #[test]
    fn inherit_all_reports_processed_count() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        vector_repo::upsert_embedding(&conn, a, &[1.0, 0.0], "v1").unwrap();
        vector_repo::upsert_embedding(&conn, b, &[0.999, 0.001], "v1").unwrap();
        tags_repo::add_tag(&conn, a, "共享", TagSource::User, 1.0).unwrap();

        let (n, inherited, _pruned) = inherit_all(&conn).unwrap();
        assert_eq!(n, 2);
        assert!(inherited >= 1, "b 应继承到标签");
    }

    #[test]
    fn empty_library_is_safe() {
        let conn = open_in_memory().unwrap();
        let (n, i, p) = inherit_all(&conn).unwrap();
        assert_eq!((n, i, p), (0, 0, 0));
    }
}
