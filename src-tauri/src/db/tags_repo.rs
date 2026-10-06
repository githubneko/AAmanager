//! `tags` 表操作。

use crate::db::audio_repo;
use crate::error::Result;
use crate::models::{Tag, TagCount, TagSource};
use rusqlite::{params, Connection, Row};
use std::collections::HashMap;

fn map_tag_row(row: &Row<'_>) -> rusqlite::Result<Tag> {
    let source_str: String = row.get(2)?;
    Ok(Tag {
        id: Some(row.get(0)?),
        tag: row.get(1)?,
        source: TagSource::from_db(&source_str),
        confidence: row.get::<_, Option<f64>>(3)?.unwrap_or(1.0) as f32,
        is_confirmed: row.get::<_, i64>(4)? != 0,
    })
}

/// 列出某个音频的全部标签。
pub fn list(conn: &Connection, audio_id: i64) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, tag, source, confidence, is_confirmed FROM tags
         WHERE audio_id = ?1
         ORDER BY (source = 'user') DESC, confidence DESC, tag ASC",
    )?;
    let rows = stmt.query_map([audio_id], map_tag_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 批量取多个音频的标签，返回 `audio_id -> tags`。
pub fn list_for_audios(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, Vec<Tag>>> {
    let mut map: HashMap<i64, Vec<Tag>> = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    // 分块查询，避免 SQLite 变量数量上限（默认 999）。
    for chunk in ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT id, tag, source, confidence, is_confirmed, audio_id FROM tags
             WHERE audio_id IN ({placeholders})
             ORDER BY (source = 'user') DESC, confidence DESC, tag ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, i64>(5)?, map_tag_row(row)?))
        })?;
        for r in rows {
            let (audio_id, tag) = r?;
            map.entry(audio_id).or_default().push(tag);
        }
    }
    Ok(map)
}

/// 添加标签（同一音频 + 标签 + 来源已存在时更新置信度），并同步 FTS。
pub fn add_tag(
    conn: &Connection,
    audio_id: i64,
    tag: &str,
    source: TagSource,
    confidence: f32,
) -> Result<()> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO tags(audio_id, tag, source, confidence, is_confirmed)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(audio_id, tag, source) DO UPDATE SET
             confidence = excluded.confidence,
             is_confirmed = MAX(tags.is_confirmed, excluded.is_confirmed)",
        params![
            audio_id,
            tag,
            source.as_db(),
            confidence.clamp(0.0, 1.0) as f64,
            i64::from(source == TagSource::User)
        ],
    )?;
    audio_repo::sync_fts(conn, audio_id)?;
    Ok(())
}

/// 批量添加同一个标签到多个音频（批量编辑），整批一个事务。
///
/// 用 `unchecked_transaction` 在 `&Connection` 上开事务，省去命令层为了
/// 批量写入而独占整个数据库锁。
pub fn add_tag_many(conn: &Connection, ids: &[i64], tag: &str, source: TagSource) -> Result<u32> {
    let tag = tag.trim();
    if tag.is_empty() || ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for id in ids {
        n += tx.execute(
            "INSERT INTO tags(audio_id, tag, source, confidence, is_confirmed)
             VALUES(?1, ?2, ?3, 1.0, ?4)
             ON CONFLICT(audio_id, tag, source) DO UPDATE SET confidence = 1.0",
            params![
                id,
                tag,
                source.as_db(),
                i64::from(source == TagSource::User)
            ],
        )? as u32;
        audio_repo::sync_fts(&tx, *id)?;
    }
    tx.commit()?;
    Ok(n)
}

/// 删除某个音频上的指定标签。
pub fn remove_tag(conn: &Connection, audio_id: i64, tag: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE audio_id = ?1 AND tag = ?2",
        params![audio_id, tag],
    )?;
    audio_repo::sync_fts(conn, audio_id)?;
    Ok(())
}

/// 删除某个音频上来自自动分析的全部标签（用于「重新分析」）。
pub fn clear_auto_tags(conn: &Connection, audio_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM tags WHERE audio_id = ?1 AND source <> 'user'",
        [audio_id],
    )?;
    audio_repo::sync_fts(conn, audio_id)?;
    Ok(())
}

/// 用户确认一个自动标签（置信度置 1.0）。
pub fn confirm_tag(conn: &Connection, audio_id: i64, tag: &str) -> Result<()> {
    conn.execute(
        "UPDATE tags SET is_confirmed = 1, confidence = 1.0 WHERE audio_id = ?1 AND tag = ?2",
        params![audio_id, tag],
    )?;
    Ok(())
}

/// 重命名标签（全局，跨所有音频）。
pub fn rename_tag(conn: &Connection, from: &str, to: &str) -> Result<u32> {
    let to = to.trim();
    if to.is_empty() {
        return Ok(0);
    }
    // 先收集受影响的音频，用于刷新 FTS
    let affected: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT DISTINCT audio_id FROM tags WHERE tag = ?1")?;
        let rows = stmt.query_map([from], |r| r.get::<_, i64>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // 存在 (audio_id, to, source) 冲突时直接替换掉旧行
    let n = conn.execute(
        "UPDATE OR REPLACE tags SET tag = ?2 WHERE tag = ?1",
        params![from, to],
    )? as u32;

    for id in affected {
        audio_repo::sync_fts(conn, id)?;
    }
    Ok(n)
}

/// 列出全部标签及使用频率（标签云）。`limit` 为 0 表示不限制。
pub fn list_all_with_counts(conn: &Connection, limit: u32) -> Result<Vec<TagCount>> {
    let sql = if limit == 0 {
        "SELECT tag, count(*) AS c, group_concat(DISTINCT source)
         FROM tags GROUP BY tag ORDER BY c DESC, tag ASC"
            .to_string()
    } else {
        format!(
            "SELECT tag, count(*) AS c, group_concat(DISTINCT source)
             FROM tags GROUP BY tag ORDER BY c DESC, tag ASC LIMIT {limit}"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let sources: Option<String> = r.get(2)?;
        Ok(TagCount {
            tag: r.get(0)?,
            count: r.get(1)?,
            sources: sources
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 给定前缀做标签自动补全。
pub fn suggest(conn: &Connection, prefix: &str, limit: u32) -> Result<Vec<String>> {
    let pattern = format!("%{}%", prefix.replace(['%', '_'], ""));
    let mut stmt = conn.prepare(
        "SELECT tag, count(*) AS c FROM tags WHERE tag LIKE ?1
         GROUP BY tag ORDER BY c DESC, tag ASC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 取某音频上所有用户标签的文本（相似度继承的源头）。
pub fn user_tags(conn: &Connection, audio_id: i64) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT tag FROM tags WHERE audio_id = ?1 AND source = 'user'")?;
    let rows = stmt.query_map([audio_id], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 所有带用户标签的音频 id（相似度继承的候选集合）。
pub fn audios_with_user_tags(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT audio_id FROM tags WHERE source = 'user' ORDER BY audio_id ASC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{audio_repo::NewAudio, open_in_memory};

    fn mk_audio(conn: &Connection, name: &str) -> i64 {
        audio_repo::insert(
            conn,
            &NewAudio {
                path: format!("C:/sfx/{name}"),
                filename: name.to_string(),
                folder: "C:/sfx".into(),
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

    #[test]
    fn add_list_remove_roundtrip() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        add_tag(&conn, id, "枪声", TagSource::User, 1.0).unwrap();
        add_tag(&conn, id, "短促", TagSource::Rule, 0.9).unwrap();

        let tags = list(&conn, id).unwrap();
        assert_eq!(tags.len(), 2);
        // 用户标签排在最前
        assert_eq!(tags[0].tag, "枪声");
        assert_eq!(tags[0].source, TagSource::User);
        assert!(tags[0].is_confirmed);
        assert!(!tags[1].is_confirmed);

        remove_tag(&conn, id, "枪声").unwrap();
        assert_eq!(list(&conn, id).unwrap().len(), 1);
    }

    #[test]
    fn duplicate_add_updates_instead_of_erroring() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        add_tag(&conn, id, "枪声", TagSource::Rule, 0.4).unwrap();
        add_tag(&conn, id, "枪声", TagSource::Rule, 0.95).unwrap();
        let tags = list(&conn, id).unwrap();
        assert_eq!(tags.len(), 1, "同源同标签应去重");
        assert!((tags[0].confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn empty_tag_is_ignored() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        add_tag(&conn, id, "   ", TagSource::User, 1.0).unwrap();
        assert!(list(&conn, id).unwrap().is_empty());
    }

    #[test]
    fn batch_add_and_clear_auto() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        add_tag_many(&conn, &[a, b], "爆炸", TagSource::User).unwrap();
        assert_eq!(list(&conn, a).unwrap().len(), 1);
        assert_eq!(list(&conn, b).unwrap().len(), 1);

        add_tag(&conn, a, "低频", TagSource::Rule, 0.8).unwrap();
        assert_eq!(list(&conn, a).unwrap().len(), 2);
        clear_auto_tags(&conn, a).unwrap();
        let left = list(&conn, a).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].tag, "爆炸");
    }

    #[test]
    fn counts_and_suggestions() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        add_tag(&conn, a, "枪声", TagSource::User, 1.0).unwrap();
        add_tag(&conn, b, "枪声", TagSource::User, 1.0).unwrap();
        add_tag(&conn, b, "枪械", TagSource::Rule, 0.7).unwrap();

        let counts = list_all_with_counts(&conn, 0).unwrap();
        assert_eq!(counts[0].tag, "枪声");
        assert_eq!(counts[0].count, 2);

        let s = suggest(&conn, "枪", 10).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], "枪声");
    }

    #[test]
    fn rename_tag_merges_into_target() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        add_tag(&conn, a, "枪声", TagSource::User, 1.0).unwrap();
        rename_tag(&conn, "枪声", "射击").unwrap();
        let tags = list(&conn, a).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "射击");
    }

    #[test]
    fn confirm_marks_tag_confirmed() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        add_tag(&conn, a, "低频", TagSource::Rule, 0.6).unwrap();
        assert!(!list(&conn, a).unwrap()[0].is_confirmed);
        confirm_tag(&conn, a, "低频").unwrap();
        let t = &list(&conn, a).unwrap()[0];
        assert!(t.is_confirmed);
        assert!((t.confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn fts_is_resynced_after_tag_change() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        add_tag(&conn, a, "枪声", TagSource::User, 1.0).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM audio_fts WHERE tags LIKE '%枪声%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        remove_tag(&conn, a, "枪声").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM audio_fts WHERE tags LIKE '%枪声%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn user_tags_and_audios_with_user_tags() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        add_tag(&conn, a, "我的枪声", TagSource::User, 1.0).unwrap();
        add_tag(&conn, a, "低频", TagSource::Rule, 0.8).unwrap();
        add_tag(&conn, b, "环境音", TagSource::User, 1.0).unwrap();

        let mine = user_tags(&conn, a).unwrap();
        assert_eq!(mine, vec!["我的枪声".to_string()], "只应返回用户标签");

        let mut with_tags = audios_with_user_tags(&conn).unwrap();
        with_tags.sort_unstable();
        assert_eq!(with_tags, vec![a, b]);
    }

    #[test]
    fn batch_add_syncs_fts_for_every_audio() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        add_tag_many(&conn, &[a, b], "爆炸", TagSource::User).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM audio_fts WHERE tags LIKE '%爆炸%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2, "两个音频的 FTS 行都应包含新标签");
    }
}
