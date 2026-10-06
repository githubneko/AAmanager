//! 数据库层：连接管理、表定义与各表的仓储操作。

pub mod audio_repo;
pub mod notes_repo;
pub mod schema;
pub mod tags_repo;
pub mod vector_repo;

use crate::error::Result;
use rusqlite::Connection;
use std::path::Path;

/// 音频文件扩展名白名单（策划书 2.1：WAV、MP3、OGG、FLAC、M4A、AIFF）。
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "wave", "mp3", "ogg", "oga", "flac", "m4a", "mp4", "aac", "aiff", "aif", "aifc", "caf",
    "opus", "wma", "mka",
];

/// 判断路径是否为受支持的音频文件。
pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 打开（必要时创建）数据库并完成建表 / 迁移。
///
/// * `path` —— SQLite 文件路径。
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;

    // WAL 提升读写并发；NORMAL 同步在桌面场景下兼顾安全与速度。
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;

    schema::migrate(&conn)?;
    Ok(conn)
}

/// 打开一个纯内存数据库（供单元测试使用）。
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    schema::migrate(&conn)?;
    Ok(conn)
}

/// 读取设置项。
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let v = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .ok();
    Ok(v)
}

/// 写入设置项。
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn supported_extensions_are_case_insensitive() {
        assert!(is_supported_audio(&PathBuf::from("a/b/Sound.WAV")));
        assert!(is_supported_audio(&PathBuf::from("kick.flac")));
        assert!(!is_supported_audio(&PathBuf::from("readme.txt")));
        assert!(!is_supported_audio(&PathBuf::from("noext")));
    }

    #[test]
    fn schema_initialises_and_is_idempotent() {
        let conn = open_in_memory().expect("open in-memory db");
        // 再次迁移不应报错。
        schema::migrate(&conn).expect("second migrate");

        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type IN ('table','view') AND name IN
                 ('audio_files','user_notes','tags','audio_embeddings','text_embedding_cache','settings','index_jobs')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 7, "核心表应全部创建");
    }

    #[test]
    fn settings_roundtrip() {
        let conn = open_in_memory().unwrap();
        assert_eq!(get_setting(&conn, "mode").unwrap(), None);
        set_setting(&conn, "mode", "lite").unwrap();
        assert_eq!(get_setting(&conn, "mode").unwrap().as_deref(), Some("lite"));
        set_setting(&conn, "mode", "full").unwrap();
        assert_eq!(get_setting(&conn, "mode").unwrap().as_deref(), Some("full"));
    }
}
