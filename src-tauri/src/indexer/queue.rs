//! 后台分析任务队列。
//!
//! 队列状态持久化在 `index_jobs` 表，程序异常退出后未完成的任务会在下次启动时续跑。

use crate::error::Result;
use crate::models::IndexStatus;
use rusqlite::{params, Connection};

/// 任务类型：目前只有分析。
pub const KIND_ANALYZE: &str = "analyze";

/// 把一个音频加入分析队列（重复入队会被 UNIQUE 约束去重）。
pub fn enqueue(conn: &Connection, audio_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO index_jobs(audio_id, kind, status) VALUES(?1, ?2, 'pending')
         ON CONFLICT(audio_id, kind) DO UPDATE SET status = 'pending', error = NULL,
             updated_at = unixepoch()",
        params![audio_id, KIND_ANALYZE],
    )?;
    Ok(())
}

/// 批量入队。
pub fn enqueue_many(conn: &mut Connection, audio_ids: &[i64]) -> Result<u32> {
    let tx = conn.transaction()?;
    let mut n = 0;
    for id in audio_ids {
        n += tx.execute(
            "INSERT INTO index_jobs(audio_id, kind, status) VALUES(?1, ?2, 'pending')
             ON CONFLICT(audio_id, kind) DO UPDATE SET status = 'pending', error = NULL,
                 updated_at = unixepoch()",
            params![id, KIND_ANALYZE],
        )? as u32;
    }
    tx.commit()?;
    Ok(n)
}

/// 取出一个待处理任务。
pub fn take_next_pending(conn: &Connection) -> Result<Option<i64>> {
    let r: Option<i64> = conn
        .query_row(
            "SELECT audio_id FROM index_jobs WHERE status = 'pending' AND kind = ?1
             ORDER BY id ASC LIMIT 1",
            [KIND_ANALYZE],
            |r| r.get(0),
        )
        .ok();
    Ok(r)
}

/// 把任务标记为运行中。
pub fn mark_running(conn: &Connection, audio_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE index_jobs SET status = 'running', updated_at = unixepoch()
         WHERE audio_id = ?1 AND kind = ?2",
        params![audio_id, KIND_ANALYZE],
    )?;
    Ok(())
}

/// 把任务标记为完成。
pub fn mark_done(conn: &Connection, audio_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE index_jobs SET status = 'done', error = NULL, updated_at = unixepoch()
         WHERE audio_id = ?1 AND kind = ?2",
        params![audio_id, KIND_ANALYZE],
    )?;
    Ok(())
}

/// 把任务标记为失败并记录原因。
pub fn mark_failed(conn: &Connection, audio_id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE index_jobs SET status = 'failed', error = ?3, updated_at = unixepoch()
         WHERE audio_id = ?1 AND kind = ?2",
        params![audio_id, KIND_ANALYZE, error],
    )?;
    Ok(())
}

/// 把启动时残留的 `running` 任务重置为 `pending`（上次异常退出的续跑）。
pub fn reset_stale_running(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE index_jobs SET status = 'pending', updated_at = unixepoch()
         WHERE status = 'running'",
        [],
    )?)
}

/// 统计各状态任务数。
pub fn status(conn: &Connection) -> Result<IndexStatus> {
    let mut s = IndexStatus::default();
    let mut stmt = conn.prepare("SELECT status, count(*) FROM index_jobs GROUP BY status")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for r in rows {
        let (st, n) = r?;
        match st.as_str() {
            "pending" => s.pending = n.max(0) as u32,
            "running" => s.running = n > 0,
            "done" => s.done = n.max(0) as u32,
            "failed" => s.failed = n.max(0) as u32,
            _ => {}
        }
    }
    Ok(s)
}

/// 清空已完成 / 失败的任务记录。
pub fn clear_finished(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM index_jobs WHERE status IN ('done','failed')",
        [],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{audio_repo::NewAudio, open_in_memory};

    fn mk_audio(conn: &Connection, name: &str) -> i64 {
        crate::db::audio_repo::insert(
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
    fn enqueue_and_drain() {
        let mut conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        let b = mk_audio(&conn, "b.wav");
        assert_eq!(enqueue_many(&mut conn, &[a, b]).unwrap(), 2);

        let st = status(&conn).unwrap();
        assert_eq!(st.pending, 2);
        assert!(!st.running);

        let next = take_next_pending(&conn).unwrap();
        assert_eq!(next, Some(a), "应按入队顺序取");
        mark_running(&conn, a).unwrap();
        assert!(status(&conn).unwrap().running);

        mark_done(&conn, a).unwrap();
        assert_eq!(take_next_pending(&conn).unwrap(), Some(b));
    }

    #[test]
    fn duplicate_enqueue_is_deduplicated() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        enqueue(&conn, a).unwrap();
        enqueue(&conn, a).unwrap();
        assert_eq!(status(&conn).unwrap().pending, 1);
    }

    #[test]
    fn stale_running_is_reset_on_startup() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        enqueue(&conn, a).unwrap();
        mark_running(&conn, a).unwrap();
        assert!(status(&conn).unwrap().running);

        let n = reset_stale_running(&conn).unwrap();
        assert_eq!(n, 1);
        let st = status(&conn).unwrap();
        assert_eq!(st.pending, 1);
        assert!(!st.running);
    }

    #[test]
    fn failure_records_reason_and_clear_removes() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        enqueue(&conn, a).unwrap();
        mark_failed(&conn, a, "解码失败").unwrap();
        assert_eq!(status(&conn).unwrap().failed, 1);

        let err: String = conn
            .query_row(
                "SELECT error FROM index_jobs WHERE audio_id = ?1",
                [a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(err, "解码失败");

        assert_eq!(clear_finished(&conn).unwrap(), 1);
        assert_eq!(status(&conn).unwrap().failed, 0);
    }

    #[test]
    fn deleting_audio_removes_its_job() {
        let conn = open_in_memory().unwrap();
        let a = mk_audio(&conn, "a.wav");
        enqueue(&conn, a).unwrap();
        crate::db::audio_repo::delete(&conn, a).unwrap();
        assert_eq!(status(&conn).unwrap().pending, 0, "级联删除应清掉任务");
    }
}
