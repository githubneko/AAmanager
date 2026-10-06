//! `user_notes` 表操作。
//!
//! 数据模型上一条音频可以有多条备注（策划书 2.1「可添加自由文本备注」），
//! 列表展示时用换行拼接为 `AudioItem::note`。

use crate::db::audio_repo;
use crate::error::Result;
use crate::models::UserNote;
use rusqlite::{params, Connection, Row};
use std::collections::HashMap;

fn map_note_row(row: &Row<'_>) -> rusqlite::Result<UserNote> {
    Ok(UserNote {
        id: row.get(0)?,
        audio_id: row.get(1)?,
        note: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

/// 列出某音频的全部备注。
pub fn list(conn: &Connection, audio_id: i64) -> Result<Vec<UserNote>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, audio_id, note, created_at, updated_at FROM user_notes
         WHERE audio_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([audio_id], map_note_row)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 批量取备注文本，返回 `audio_id -> 拼接后的文本`。
pub fn notes_for_audios(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut map: HashMap<i64, String> = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    for chunk in ids.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT audio_id, note FROM user_notes WHERE audio_id IN ({placeholders}) ORDER BY id ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        for r in rows {
            let (id, note) = r?;
            let entry = map.entry(id).or_default();
            if !entry.is_empty() {
                entry.push('\n');
            }
            entry.push_str(&note);
        }
    }
    Ok(map)
}

/// 用一段文本替换某音频的全部备注（空字符串则清空）。同步 FTS。
pub fn set_note(conn: &Connection, audio_id: i64, note: &str) -> Result<()> {
    conn.execute("DELETE FROM user_notes WHERE audio_id = ?1", [audio_id])?;
    let trimmed = note.trim();
    if !trimmed.is_empty() {
        conn.execute(
            "INSERT INTO user_notes(audio_id, note) VALUES(?1, ?2)",
            params![audio_id, trimmed],
        )?;
    }
    audio_repo::sync_fts(conn, audio_id)?;
    Ok(())
}

/// 在现有备注后追加一行（不覆盖）。
pub fn append_note(conn: &Connection, audio_id: i64, note: &str) -> Result<i64> {
    let trimmed = note.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    conn.execute(
        "INSERT INTO user_notes(audio_id, note) VALUES(?1, ?2)",
        params![audio_id, trimmed],
    )?;
    let id = conn.last_insert_rowid();
    audio_repo::sync_fts(conn, audio_id)?;
    Ok(id)
}

/// 批量设置同一段备注到多个音频，整批一个事务。
pub fn set_note_many(conn: &Connection, ids: &[i64], note: &str) -> Result<u32> {
    let tx = conn.unchecked_transaction()?;
    let mut n = 0;
    for id in ids {
        tx.execute("DELETE FROM user_notes WHERE audio_id = ?1", [id])?;
        let trimmed = note.trim();
        if !trimmed.is_empty() {
            tx.execute(
                "INSERT INTO user_notes(audio_id, note) VALUES(?1, ?2)",
                params![id, trimmed],
            )?;
        }
        audio_repo::sync_fts(&tx, *id)?;
        n += 1;
    }
    tx.commit()?;
    Ok(n)
}

/// 删除单条备注。
pub fn delete(conn: &Connection, note_id: i64) -> Result<()> {
    let audio_id: Option<i64> = conn
        .query_row(
            "SELECT audio_id FROM user_notes WHERE id = ?1",
            [note_id],
            |r| r.get(0),
        )
        .ok();
    conn.execute("DELETE FROM user_notes WHERE id = ?1", [note_id])?;
    if let Some(id) = audio_id {
        audio_repo::sync_fts(conn, id)?;
    }
    Ok(())
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
    fn set_note_replaces_previous() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        set_note(&conn, id, "第一版").unwrap();
        assert_eq!(list(&conn, id).unwrap().len(), 1);
        set_note(&conn, id, "第二版").unwrap();
        let notes = list(&conn, id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].note, "第二版");
    }

    #[test]
    fn empty_note_clears() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        set_note(&conn, id, "有内容").unwrap();
        set_note(&conn, id, "   ").unwrap();
        assert!(list(&conn, id).unwrap().is_empty());
    }

    #[test]
    fn append_keeps_multiple_lines() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        append_note(&conn, id, "行一").unwrap();
        append_note(&conn, id, "行二").unwrap();
        assert_eq!(list(&conn, id).unwrap().len(), 2);
        let map = notes_for_audios(&conn, &[id]).unwrap();
        assert_eq!(map.get(&id).unwrap(), "行一\n行二");
    }

    #[test]
    fn note_participates_in_fts() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        set_note(&conn, id, "远处的雷声").unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT count(*) FROM audio_fts WHERE user_notes LIKE '%雷声%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);
    }

    #[test]
    fn batch_set_and_delete() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        assert_eq!(set_note_many(&conn, &[a, b], "统一备注").unwrap(), 2);
        assert_eq!(list(&conn, a).unwrap()[0].note, "统一备注");
        assert_eq!(list(&conn, b).unwrap()[0].note, "统一备注");

        let note_id = list(&conn, a).unwrap()[0].id;
        delete(&conn, note_id).unwrap();
        assert!(list(&conn, a).unwrap().is_empty());
        assert_eq!(list(&conn, b).unwrap().len(), 1, "不应误删其它音频的备注");
    }
}
