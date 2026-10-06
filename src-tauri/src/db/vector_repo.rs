//! `audio_embeddings` / `text_embedding_cache` 表操作。
//!
//! 向量以 f32 小端连续字节存储于 BLOB。Phase 1 只提供存取与缓存能力，
//! 推理侧（CLAP）按用户决定暂缓，接入时无需改动本模块。

use crate::error::{AppError, Result};
use rusqlite::{params, Connection};

/// 把 f32 切片编码为小端字节串。
pub fn encode(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for v in vec {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 把字节串解码为 f32 向量。
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(AppError::Invalid(format!(
            "向量字节长度 {} 不是 4 的倍数",
            bytes.len()
        )));
    }
    let (chunks, _rest) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// 保存 / 覆盖某个音频的向量。
pub fn upsert_embedding(
    conn: &Connection,
    audio_id: i64,
    embedding: &[f32],
    model_version: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_embeddings(audio_id, embedding, dim, model_version)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(audio_id) DO UPDATE SET
             embedding = excluded.embedding,
             dim = excluded.dim,
             model_version = excluded.model_version",
        params![
            audio_id,
            encode(embedding),
            embedding.len() as i64,
            model_version
        ],
    )?;
    Ok(())
}

/// 读取某个音频的向量。
pub fn get_embedding(conn: &Connection, audio_id: i64) -> Result<Option<Vec<f32>>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM audio_embeddings WHERE audio_id = ?1",
            [audio_id],
            |r| r.get(0),
        )
        .ok();
    match blob {
        Some(b) => Ok(Some(decode(&b)?)),
        None => Ok(None),
    }
}

/// 取出全部音频向量。
pub fn all_embeddings(conn: &Connection) -> Result<Vec<(i64, Vec<f32>)>> {
    let mut stmt = conn.prepare("SELECT audio_id, embedding FROM audio_embeddings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        let (id, blob) = r?;
        out.push((id, decode(&blob)?));
    }
    Ok(out)
}

/// 已生成向量的音频数量。
pub fn count_embeddings(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT count(*) FROM audio_embeddings", [], |r| r.get(0))?)
}

/// 命中缓存的文本向量。
///
/// 必须带 `model_version`：换模型后旧向量与新查询不可比，
/// 不加过滤会把上一个模型的向量当成当前模型的用。
pub fn get_text_embedding(
    conn: &Connection,
    text: &str,
    model_version: &str,
) -> Result<Option<Vec<f32>>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM text_embedding_cache
             WHERE text = ?1 AND model_version = ?2",
            params![text, model_version],
            |r| r.get(0),
        )
        .ok();
    match blob {
        Some(b) => Ok(Some(decode(&b)?)),
        None => Ok(None),
    }
}

/// 写入文本向量缓存。
pub fn cache_text_embedding(
    conn: &Connection,
    text: &str,
    embedding: &[f32],
    model_version: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO text_embedding_cache(text, model_version, embedding, dim)
         VALUES(?1, ?2, ?3, ?4)
         ON CONFLICT(text, model_version) DO UPDATE SET
             embedding = excluded.embedding,
             dim = excluded.dim",
        params![
            text,
            model_version,
            encode(embedding),
            embedding.len() as i64
        ],
    )?;
    Ok(())
}

/// 清空文本向量缓存（切换模型版本时使用）。
pub fn clear_text_cache(conn: &Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM text_embedding_cache", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::audio_repo::{self, NewAudio};
    use crate::db::open_in_memory;

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
    fn encode_decode_roundtrip() {
        let v = vec![0.0f32, 1.5, -2.25, f32::MIN, f32::MAX];
        let bytes = encode(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        assert_eq!(decode(&bytes).unwrap(), v);
    }

    #[test]
    fn decode_rejects_misaligned_bytes() {
        assert!(decode(&[0u8, 1, 2]).is_err());
    }

    #[test]
    fn embedding_upsert_and_read() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        assert!(get_embedding(&conn, id).unwrap().is_none());

        upsert_embedding(&conn, id, &[0.1, 0.2, 0.3], "clap-v1").unwrap();
        let got = get_embedding(&conn, id).unwrap().unwrap();
        assert_eq!(got.len(), 3);

        // 覆盖写
        upsert_embedding(&conn, id, &[1.0, 2.0], "clap-v1").unwrap();
        assert_eq!(get_embedding(&conn, id).unwrap().unwrap().len(), 2);
        assert_eq!(count_embeddings(&conn).unwrap(), 1);

        let all = all_embeddings(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, id);
    }

    #[test]
    fn text_cache_roundtrip() {
        let conn = open_in_memory().unwrap();
        assert!(get_text_embedding(&conn, "沉闷的撞击声", "clap-v1")
            .unwrap()
            .is_none());
        cache_text_embedding(&conn, "沉闷的撞击声", &[0.5, 0.25], "clap-v1").unwrap();
        assert_eq!(
            get_text_embedding(&conn, "沉闷的撞击声", "clap-v1")
                .unwrap()
                .unwrap(),
            vec![0.5, 0.25]
        );
        assert_eq!(clear_text_cache(&conn).unwrap(), 1);
        assert!(get_text_embedding(&conn, "沉闷的撞击声", "clap-v1")
            .unwrap()
            .is_none());
    }

    /// 换模型版本后旧缓存的向量不能被当成新模型的用 —— 两者不可比。
    #[test]
    fn text_cache_is_scoped_by_model_version() {
        let conn = open_in_memory().unwrap();
        cache_text_embedding(&conn, "枪声", &[1.0, 0.0], "clap-v1").unwrap();

        // 同版本命中
        assert!(get_text_embedding(&conn, "枪声", "clap-v1")
            .unwrap()
            .is_some());
        // 换版本不命中
        assert!(get_text_embedding(&conn, "枪声", "clap-v2")
            .unwrap()
            .is_none());

        // 新版本写入后两者各自独立
        cache_text_embedding(&conn, "枪声", &[0.0, 1.0], "clap-v2").unwrap();
        assert_eq!(
            get_text_embedding(&conn, "枪声", "clap-v1")
                .unwrap()
                .unwrap(),
            vec![1.0, 0.0]
        );
        assert_eq!(
            get_text_embedding(&conn, "枪声", "clap-v2")
                .unwrap()
                .unwrap(),
            vec![0.0, 1.0]
        );
    }

    #[test]
    fn deleting_audio_cascades_embedding() {
        let conn = open_in_memory().unwrap();
        let id = mk_audio(&conn, "a.wav");
        upsert_embedding(&conn, id, &[1.0], "clap-v1").unwrap();
        crate::db::audio_repo::delete(&conn, id).unwrap();
        assert_eq!(count_embeddings(&conn).unwrap(), 0);
    }
}
