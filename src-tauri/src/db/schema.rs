//! 数据库表定义与迁移。
//!
//! 采用「启动时幂等建表 + `schema_version` 记录版本」的方式做迁移，
//! 与策划书 4.2 节的 Schema 保持一致，并补充了后台索引任务表。

use rusqlite::{Connection, Result};

/// 当前 Schema 版本。每次结构变更时 +1，并在 [`migrate`] 中追加迁移步骤。
pub const SCHEMA_VERSION: i64 = 1;

/// 建表语句集合。
const CREATE_STATEMENTS: &[&str] = &[
    // ---------- 音频文件表 ----------
    r#"
    CREATE TABLE IF NOT EXISTS audio_files (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        path          TEXT UNIQUE NOT NULL,
        filename      TEXT NOT NULL,
        folder        TEXT,
        duration      REAL,
        sample_rate   INTEGER,
        channels      INTEGER,
        format        TEXT,
        bitrate       INTEGER,
        file_size     INTEGER,
        file_hash     TEXT,
        waveform_data BLOB,
        created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at    INTEGER NOT NULL DEFAULT (unixepoch())
    );
    "#,
    "CREATE INDEX IF NOT EXISTS idx_audio_hash ON audio_files(file_hash);",
    "CREATE INDEX IF NOT EXISTS idx_audio_folder ON audio_files(folder);",
    "CREATE INDEX IF NOT EXISTS idx_audio_format ON audio_files(format);",
    "CREATE INDEX IF NOT EXISTS idx_audio_duration ON audio_files(duration);",
    // ---------- 用户备注表 ----------
    r#"
    CREATE TABLE IF NOT EXISTS user_notes (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        audio_id   INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
        note       TEXT NOT NULL,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch())
    );
    "#,
    "CREATE INDEX IF NOT EXISTS idx_notes_audio ON user_notes(audio_id);",
    // ---------- 标签表（用户标签 + 自动标签统一存储） ----------
    r#"
    CREATE TABLE IF NOT EXISTS tags (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        audio_id     INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
        tag          TEXT NOT NULL,
        source       TEXT NOT NULL,
        confidence   REAL,
        is_confirmed INTEGER NOT NULL DEFAULT 0,
        created_at   INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE(audio_id, tag, source)
    );
    "#,
    "CREATE INDEX IF NOT EXISTS idx_tags_audio ON tags(audio_id);",
    "CREATE INDEX IF NOT EXISTS idx_tags_tag ON tags(tag);",
    "CREATE INDEX IF NOT EXISTS idx_tags_source ON tags(source);",
    // ---------- 音频向量表（CLAP 嵌入，Phase 5 使用） ----------
    r#"
    CREATE TABLE IF NOT EXISTS audio_embeddings (
        audio_id      INTEGER PRIMARY KEY REFERENCES audio_files(id) ON DELETE CASCADE,
        embedding     BLOB NOT NULL,
        dim           INTEGER NOT NULL DEFAULT 512,
        model_version TEXT,
        created_at    INTEGER NOT NULL DEFAULT (unixepoch())
    );
    "#,
    // ---------- 文本向量缓存 ----------
    // 主键是 (text, model_version)：换模型后旧向量与新查询不可比，
    // 若只用 text 作主键，切换模型时会读到上一个模型的向量。
    r#"
    CREATE TABLE IF NOT EXISTS text_embedding_cache (
        text          TEXT NOT NULL,
        model_version TEXT NOT NULL,
        embedding     BLOB NOT NULL,
        dim           INTEGER NOT NULL DEFAULT 512,
        created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
        PRIMARY KEY (text, model_version)
    );
    "#,
    // ---------- 全文搜索索引 ----------
    // 说明：策划书原文使用 content=''（无内容表），但无内容 FTS5 表无法稳定执行
    // UPDATE/DELETE，而标签和备注是持续变化的。这里改用普通 FTS5 表，由
    // [crate::db::audio_repo] 在写入时显式同步，删除时按 rowid 精确删除，
    // 语义等价且可维护。
    r#"
    CREATE VIRTUAL TABLE IF NOT EXISTS audio_fts USING fts5(
        filename,
        user_notes,
        tags,
        tokenize = 'unicode61 remove_diacritics 2'
    );
    "#,
    // ---------- 后台索引任务表 ----------
    // 让未完成的自动分析任务在重启后可以续跑。
    r#"
    CREATE TABLE IF NOT EXISTS index_jobs (
        id         INTEGER PRIMARY KEY AUTOINCREMENT,
        audio_id   INTEGER NOT NULL REFERENCES audio_files(id) ON DELETE CASCADE,
        kind       TEXT NOT NULL DEFAULT 'analyze',
        status     TEXT NOT NULL DEFAULT 'pending',
        error      TEXT,
        created_at INTEGER NOT NULL DEFAULT (unixepoch()),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
        UNIQUE(audio_id, kind)
    );
    "#,
    "CREATE INDEX IF NOT EXISTS idx_jobs_status ON index_jobs(status);",
    // ---------- 设置表 ----------
    r#"
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
];

/// 创建全部表与索引，并记录版本号。可重复调用。
pub fn init(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    for stmt in CREATE_STATEMENTS {
        conn.execute_batch(stmt)?;
    }
    conn.execute(
        "INSERT INTO settings(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// 执行迁移。当前仅支持版本 1，后续版本在此追加分支。
pub fn migrate(conn: &Connection) -> Result<()> {
    let current: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();

    match current.as_deref().and_then(|v| v.parse::<i64>().ok()) {
        // 全新数据库或已是当前版本。
        None | Some(SCHEMA_VERSION) => init(conn),
        // 未来：Some(v) if v < SCHEMA_VERSION => { ...逐步升级... }
        Some(v) => {
            log::warn!("数据库版本 {v} 与当前 {SCHEMA_VERSION} 不一致，执行重建索引");
            init(conn)
        }
    }
}

/// 确保 FTS 索引与外键表数据一致（用于首次升级或索引损坏后的修复）。
pub fn rebuild_fts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM audio_fts;
         INSERT INTO audio_fts(rowid, filename, user_notes, tags)
         SELECT a.id,
                a.filename,
                COALESCE((SELECT group_concat(n.note, ' ') FROM user_notes n WHERE n.audio_id = a.id), ''),
                COALESCE((SELECT group_concat(t.tag, ' ')  FROM tags t       WHERE t.audio_id = a.id), '')
         FROM audio_files a;",
    )
}
