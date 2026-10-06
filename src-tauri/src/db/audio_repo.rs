//! `audio_files` 表操作，以及关键词 / 筛选 / 排序 / 分页查询。
//!
//! 查询使用动态 SQL 拼接，但**所有用户输入一律走 `?` 占位符绑定**，
//! 只有经过白名单映射的列名与固定片段才会拼接进 SQL。

use crate::db::{notes_repo, tags_repo};
use crate::error::{AppError, Result};
use crate::models::{AudioFile, AudioItem, DurationBucket, SearchQuery, SortBy};
use rusqlite::{params, params_from_iter, Connection, Row};
use std::collections::HashMap;

/// `audio_files` 的标准查询列（顺序与 [`map_audio_row`] 一致）。
const AUDIO_COLUMNS: &str = "a.id, a.path, a.filename, a.duration, a.sample_rate, a.channels, \
                             a.format, a.bitrate, a.file_size, a.file_hash, a.created_at, a.updated_at";

/// 新导入音频时需要落库的字段。
#[derive(Debug, Clone)]
pub struct NewAudio {
    /// 磁盘绝对路径。
    pub path: String,
    /// 文件名。
    pub filename: String,
    /// 所在文件夹。
    pub folder: String,
    /// 时长（秒）。
    pub duration: Option<f64>,
    /// 采样率。
    pub sample_rate: Option<i64>,
    /// 声道数。
    pub channels: Option<i64>,
    /// 格式。
    pub format: Option<String>,
    /// 码率。
    pub bitrate: Option<i64>,
    /// 文件大小。
    pub file_size: Option<i64>,
    /// 内容哈希。
    pub file_hash: Option<String>,
}

/// 将结果集行映射为 [`AudioFile`]。
fn map_audio_row(row: &Row<'_>) -> rusqlite::Result<AudioFile> {
    Ok(AudioFile {
        id: row.get(0)?,
        path: row.get(1)?,
        filename: row.get(2)?,
        duration: row.get(3)?,
        sample_rate: row.get(4)?,
        channels: row.get(5)?,
        format: row.get(6)?,
        bitrate: row.get(7)?,
        file_size: row.get(8)?,
        file_hash: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

/// 按路径查询音频，不存在返回 `None`。
pub fn find_by_path(conn: &Connection, path: &str) -> Result<Option<AudioFile>> {
    let sql = format!("SELECT {AUDIO_COLUMNS} FROM audio_files a WHERE a.path = ?1");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query_map([path], map_audio_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 按哈希查询音频（用于跨路径去重），不存在返回 `None`。
pub fn find_by_hash(conn: &Connection, hash: &str) -> Result<Option<AudioFile>> {
    let sql = format!("SELECT {AUDIO_COLUMNS} FROM audio_files a WHERE a.file_hash = ?1 LIMIT 1");
    let mut stmt = conn.prepare_cached(&sql)?;
    let mut rows = stmt.query_map([hash], map_audio_row)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// 按 id 查询音频。
pub fn get(conn: &Connection, id: i64) -> Result<AudioFile> {
    let sql = format!("SELECT {AUDIO_COLUMNS} FROM audio_files a WHERE a.id = ?1");
    conn.query_row(&sql, [id], map_audio_row)
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound(format!("音频 #{id}")),
            other => AppError::Database(other),
        })
}

/// 插入一条音频记录，返回新 id，并同步 FTS 索引。
pub fn insert(conn: &Connection, new: &NewAudio) -> Result<i64> {
    conn.execute(
        "INSERT INTO audio_files
            (path, filename, folder, duration, sample_rate, channels, format, bitrate, file_size, file_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            new.path,
            new.filename,
            new.folder,
            new.duration,
            new.sample_rate,
            new.channels,
            new.format,
            new.bitrate,
            new.file_size,
            new.file_hash,
        ],
    )?;
    let id = conn.last_insert_rowid();
    sync_fts(conn, id)?;
    Ok(id)
}

/// 批量插入，整个批次放在一个事务里，保证「1000 文件导入」的目标。
///
/// 返回成功插入的 id 列表。
pub fn insert_many(conn: &mut Connection, items: &[NewAudio]) -> Result<Vec<i64>> {
    let tx = conn.transaction()?;
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        tx.execute(
            "INSERT INTO audio_files
                (path, filename, folder, duration, sample_rate, channels, format, bitrate, file_size, file_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                item.path,
                item.filename,
                item.folder,
                item.duration,
                item.sample_rate,
                item.channels,
                item.format,
                item.bitrate,
                item.file_size,
                item.file_hash,
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO audio_fts(rowid, filename, user_notes, tags) VALUES(?1, ?2, '', '')",
            params![id, item.filename],
        )?;
        ids.push(id);
    }
    tx.commit()?;
    Ok(ids)
}

/// 更新音频元数据字段。
///
/// 参数较多是因为它们一一对应表字段，抽成结构体反而增加调用方的噪音。
#[allow(clippy::too_many_arguments)]
pub fn update_metadata(
    conn: &Connection,
    id: i64,
    duration: Option<f64>,
    sample_rate: Option<i64>,
    channels: Option<i64>,
    format: Option<&str>,
    bitrate: Option<i64>,
    file_size: Option<i64>,
) -> Result<()> {
    conn.execute(
        "UPDATE audio_files SET duration = ?2, sample_rate = ?3, channels = ?4, format = ?5,
                bitrate = ?6, file_size = ?7, updated_at = unixepoch()
         WHERE id = ?1",
        params![
            id,
            duration,
            sample_rate,
            channels,
            format,
            bitrate,
            file_size
        ],
    )?;
    Ok(())
}

/// 更新文件哈希。
pub fn update_hash(conn: &Connection, id: i64, hash: &str) -> Result<()> {
    conn.execute(
        "UPDATE audio_files SET file_hash = ?2, updated_at = unixepoch() WHERE id = ?1",
        params![id, hash],
    )?;
    Ok(())
}

/// 删除音频（标签、备注、向量随外键级联删除），同时清理 FTS。
pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM audio_fts WHERE rowid = ?1", [id])?;
    conn.execute("DELETE FROM audio_files WHERE id = ?1", [id])?;
    Ok(())
}

/// 批量删除。
pub fn delete_many(conn: &mut Connection, ids: &[i64]) -> Result<u32> {
    let tx = conn.transaction()?;
    let mut n = 0;
    for id in ids {
        tx.execute("DELETE FROM audio_fts WHERE rowid = ?1", [id])?;
        n += tx.execute("DELETE FROM audio_files WHERE id = ?1", [id])? as u32;
    }
    tx.commit()?;
    Ok(n)
}

/// 重命名磁盘文件后同步数据库路径。
pub fn update_path(
    conn: &Connection,
    id: i64,
    path: &str,
    filename: &str,
    folder: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE audio_files SET path = ?2, filename = ?3, folder = ?4, updated_at = unixepoch()
         WHERE id = ?1",
        params![id, path, filename, folder],
    )?;
    sync_fts(conn, id)?;
    Ok(())
}

/// 保存波形峰值数据（每字节一个 0-255 的峰值）。
pub fn save_waveform(conn: &Connection, id: i64, peaks: &[u8]) -> Result<()> {
    conn.execute(
        "UPDATE audio_files SET waveform_data = ?2, updated_at = unixepoch() WHERE id = ?1",
        params![id, peaks],
    )?;
    Ok(())
}

/// 读取波形峰值数据。
pub fn get_waveform(conn: &Connection, id: i64) -> Result<Vec<u8>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT waveform_data FROM audio_files WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound(format!("音频 #{id}")),
            other => AppError::Database(other),
        })?;
    Ok(blob.unwrap_or_default())
}

/// 重新计算并写回单个音频的 FTS 行。
pub fn sync_fts(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM audio_fts WHERE rowid = ?1", [id])?;
    conn.execute(
        "INSERT INTO audio_fts(rowid, filename, user_notes, tags)
         SELECT a.id,
                a.filename,
                COALESCE((SELECT group_concat(n.note, ' ') FROM user_notes n WHERE n.audio_id = a.id), ''),
                COALESCE((SELECT group_concat(t.tag, ' ')  FROM tags t       WHERE t.audio_id = a.id), '')
         FROM audio_files a WHERE a.id = ?1",
        [id],
    )?;
    Ok(())
}

/// 用 FTS 检索命中的音频 id 集合（不排序，排序交给主查询）。
fn fts_match_ids(conn: &Connection, match_expr: &str, limit: u32) -> Result<Vec<i64>> {
    if match_expr.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(
        "SELECT rowid FROM audio_fts WHERE audio_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![match_expr, limit], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 用 LIKE 兜底检索（中文等 CJK 查询在 unicode61 分词下无法被 MATCH 命中）。
fn like_match_ids(conn: &Connection, keyword: &str, limit: u32) -> Result<Vec<i64>> {
    let pattern = format!("%{}%", keyword.replace(['%', '_'], ""));
    let mut stmt = conn.prepare_cached(
        "SELECT a.id FROM audio_files a
         WHERE a.filename LIKE ?1
            OR EXISTS (SELECT 1 FROM tags t       WHERE t.audio_id = a.id AND t.tag  LIKE ?1)
            OR EXISTS (SELECT 1 FROM user_notes n WHERE n.audio_id = a.id AND n.note LIKE ?1)
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![pattern, limit], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 把一批关键词编译成 FTS5 MATCH 表达式。
///
/// 每个词都会被加双引号成为短语，内部双引号翻倍转义，避免用户输入被当作
/// FTS5 语法（例如 `*`、`NEAR`、`(`）导致语法错误或注入。
pub fn build_match_expr(keywords: &[String], mode: crate::models::KeywordMode) -> String {
    use crate::models::KeywordMode;
    let joiner = match mode {
        KeywordMode::And => " AND ",
        KeywordMode::Or => " OR ",
    };
    keywords
        .iter()
        .map(|k| format!("\"{}\"", k.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(joiner)
}

/// 检索命中的结果，区分「强命中」与「仅单字命中」。
///
/// 中文切词会产生单字（「沉闷的爆炸」里有「闷」「炸」）。单字命中几乎是噪声：
/// 任何含该字的标签都会进来，把真正相关的结果冲淡。因此单独归到 `weak`：
/// 只有在**没有强命中**时才使用它们（例如用户就搜了一个字「鼓」）。
#[derive(Debug, Default)]
pub struct MatchResult {
    /// 由多字词 / 二元组 / 英文词命中的音频，附命中数（降序）。
    pub strong: Vec<(i64, u32)>,
    /// 仅由单个汉字命中的音频。
    pub weak: Vec<i64>,
}

impl MatchResult {
    /// 优先取强命中；没有强命中时才退回弱命中。
    pub fn effective_ids(&self) -> Vec<i64> {
        let mut out: Vec<i64> = self.strong.iter().map(|(id, _)| *id).collect();
        if out.is_empty() {
            out = self.weak.clone();
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// 用于相关性排序的命中数（弱命中记 1）。
    pub fn rank_of(&self, id: i64) -> u32 {
        self.strong
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| *n)
            .unwrap_or(1)
    }
}

/// 关键词检索：FTS5 优先，CJK / 短词用 LIKE 兜底，再按 mode 求交或求并。
///
/// 返回命中的音频 id（顺序无意义）。
///
/// **注意 mode 的语义**：`And` 表示"每个关键词都要命中"。由于切词会把中文
/// 拆成单字 + 二元组（见 [`crate::text::tokenize`]），其中必然包含不存在的
/// 片段（如「沉闷的爆炸」里的「闷的」「的爆」），因此对**整串查询**使用 `And`
/// 是不可行的 —— 调用方在词汇展开路径上应当传 `Or`，再按命中关键词数量
/// 排序（见 [`keyword_match_ids_ranked`]）。
pub fn keyword_match_ids(
    conn: &Connection,
    keywords: &[String],
    mode: crate::models::KeywordMode,
    limit: u32,
) -> Result<Vec<i64>> {
    Ok(keyword_match_ids_ranked(conn, keywords, mode, limit)?.effective_ids())
}

/// 是否为「仅单字」的关键词（单个汉字）。
///
/// 这类词的命中视为弱命中：中文切词会产出单字（「沉闷的爆炸」里有「闷」「炸」），
/// 而单字几乎能匹配到任何含该字的标签，把真正相关的结果冲淡。
fn is_single_cjk(k: &str) -> bool {
    let mut it = k.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => crate::text::is_cjk(c),
        _ => false,
    }
}

/// 同 [`keyword_match_ids`]，但区分强命中与「仅单字」弱命中，并返回命中数。
///
/// 命中数用于在词汇展开后的并集里排序：命中的关键词越多，
/// 越可能对应查询的真实意图（例如「沉闷的爆炸」里同时命中
/// 「沉闷」与「爆炸」的那条音频应排在只命中一个的前面）。
pub fn keyword_match_ids_ranked(
    conn: &Connection,
    keywords: &[String],
    mode: crate::models::KeywordMode,
    limit: u32,
) -> Result<MatchResult> {
    use crate::models::KeywordMode;
    use std::collections::{HashMap, HashSet};

    if keywords.is_empty() {
        return Ok(MatchResult::default());
    }

    // 单字与非单字分开：单字命中算弱命中（见 `MatchResult`）
    let (strong_kws, weak_kws): (Vec<String>, Vec<String>) =
        keywords.iter().cloned().partition(|k| !is_single_cjk(k));

    let mut strong_hits: HashMap<i64, u32> = HashMap::new();
    let mut weak_hits: HashSet<i64> = HashSet::new();

    // ---- 多字关键词：直接子串匹配（LIKE）----
    //
    // 这里**刻意以 LIKE 为主、FTS 为辅**。原因：FTS5 的 unicode61 分词器把
    // 中文按**单字**切分，索引里存的是「宽 带 噪 声」。于是查询「爆炸」会被
    // 变成短语 `"爆炸"` -> 词元 [爆, 炸]，只要文档里有相邻的「爆」「炸」
    // 就算命中 —— 而「宽带噪声」里并没有「爆炸」，却因为分词方式被误命中
    // （实测到 `ambient_long.ogg` 因「宽带噪声」混进「爆炸」的结果里）。
    //
    // LIKE 子串匹配没有这个问题：它匹配的是**真实文本**，语义精确。
    let per_keyword: Vec<HashSet<i64>> = strong_kws
        .iter()
        .map(|k| like_match_ids(conn, k, limit).map(|v| v.into_iter().collect()))
        .collect::<Result<Vec<_>>>()?;

    match mode {
        KeywordMode::And => {
            // 求交：每个关键词都要命中
            let acc: HashSet<i64> = match per_keyword.split_first() {
                None => HashSet::new(),
                Some((first, rest)) => {
                    let mut a = first.clone();
                    for set in rest {
                        a = a.intersection(set).copied().collect();
                    }
                    a
                }
            };
            if !acc.is_empty() {
                let mut strong: Vec<(i64, u32)> = acc
                    .into_iter()
                    .map(|id| (id, strong_kws.len().min(u32::MAX as usize) as u32))
                    .collect();
                strong.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                return Ok(MatchResult {
                    strong,
                    weak: Vec::new(),
                });
            }
            // 交集为空：中文切词会产生不存在的片段（「闷的」「的爆」），
            // 用户并非真要同时满足所有片段，退回 OR 更有用。
        }
        KeywordMode::Or => {}
    }

    // 求并，并累计每个音频命中了几个关键词
    for set in &per_keyword {
        for id in set {
            *strong_hits.entry(*id).or_insert(0) += 1;
        }
    }

    // FTS 只作为**补充**：仅在 LIKE 一条都没命中时启用。
    // 这样既保留了 FTS 的排序能力，又不会被它的中文分词噪声污染。
    if strong_hits.is_empty() {
        let fts_expr = build_match_expr(&strong_kws, mode);
        for id in fts_match_ids(conn, &fts_expr, limit).unwrap_or_default() {
            *strong_hits.entry(id).or_insert(0) += 1;
        }
    }

    // 单字单独查，且只进弱集合
    for k in &weak_kws {
        if let Ok(ids) = like_match_ids(conn, k, limit) {
            for id in ids {
                weak_hits.insert(id);
            }
        }
    }

    let mut strong: Vec<(i64, u32)> = strong_hits.into_iter().collect();
    strong.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut weak: Vec<i64> = weak_hits
        .into_iter()
        .filter(|id| !strong.iter().any(|(sid, _)| *sid == *id))
        .collect();
    weak.sort_unstable();

    Ok(MatchResult { strong, weak })
}

/// 按查询条件组装 SQL 的 WHERE 子句与绑定参数。
fn build_filters(
    q: &SearchQuery,
    candidate_ids: Option<&[i64]>,
) -> (Vec<String>, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ids) = candidate_ids {
        if ids.is_empty() {
            // 明确不可能命中的条件，避免 IN () 语法错误。
            clauses.push("1 = 0".to_string());
        } else {
            let placeholders = vec!["?"; ids.len()].join(",");
            clauses.push(format!("a.id IN ({placeholders})"));
            for id in ids {
                args.push(Box::new(*id));
            }
        }
    }

    if let Some(folder) = q.folder.as_ref().filter(|f| !f.is_empty()) {
        clauses.push("a.folder = ?".to_string());
        args.push(Box::new(folder.clone()));
    }

    match q.duration {
        DurationBucket::All => {}
        DurationBucket::Lt1 => {
            clauses.push("a.duration IS NOT NULL AND a.duration < 1.0".to_string())
        }
        DurationBucket::B1To5 => clauses
            .push("a.duration IS NOT NULL AND a.duration >= 1.0 AND a.duration <= 5.0".to_string()),
        DurationBucket::Gt5 => {
            clauses.push("a.duration IS NOT NULL AND a.duration > 5.0".to_string())
        }
    }

    if !q.formats.is_empty() {
        let placeholders = vec!["?"; q.formats.len()].join(",");
        clauses.push(format!("a.format IN ({placeholders})"));
        for f in &q.formats {
            args.push(Box::new(f.to_ascii_lowercase()));
        }
    }

    if !q.sources.is_empty() {
        let placeholders = vec!["?"; q.sources.len()].join(",");
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM tags ts WHERE ts.audio_id = a.id AND ts.source IN ({placeholders}))"
        ));
        for s in &q.sources {
            args.push(Box::new(s.to_ascii_lowercase()));
        }
    }

    if q.pending_only {
        clauses.push(
            "EXISTS (SELECT 1 FROM tags tp WHERE tp.audio_id = a.id AND tp.is_confirmed = 0 AND tp.source <> 'user')"
                .to_string(),
        );
    }

    (clauses, args)
}

/// 将排序枚举映射为 SQL 片段（白名单，杜绝注入）。
fn order_clause(sort_by: SortBy, descending: bool) -> &'static str {
    match (sort_by, descending) {
        (SortBy::Name, false) => "ORDER BY a.filename COLLATE NOCASE ASC",
        (SortBy::Name, true) => "ORDER BY a.filename COLLATE NOCASE DESC",
        (SortBy::Duration, false) => "ORDER BY a.duration IS NULL, a.duration ASC",
        (SortBy::Duration, true) => "ORDER BY a.duration IS NULL, a.duration DESC",
        (SortBy::ImportedAt, false) => "ORDER BY a.created_at ASC, a.id ASC",
        (SortBy::ImportedAt, true) => "ORDER BY a.created_at DESC, a.id DESC",
        // 相似度排序由语义检索路径单独处理，此处退化为导入时间。
        (SortBy::Similarity, false) => "ORDER BY a.created_at ASC, a.id ASC",
        (SortBy::Similarity, true) => "ORDER BY a.created_at DESC, a.id DESC",
    }
}

/// 按查询条件检索音频列表（含标签与备注）。
///
/// 返回 `(条目, 命中总数)`。
pub fn query(conn: &Connection, q: &SearchQuery) -> Result<(Vec<AudioItem>, u32)> {
    // 1. 关键词 -> 候选 id
    //
    // 切词走 [`crate::text::tokenize`]：中文会同时产出单字与二元组，
    // 因此「沉闷的爆炸」能拆出「沉闷」「爆炸」命中自带标签，
    // 而不再是去找不存在的连续串「沉闷的爆炸」。
    //
    // 但**单字要从检索词里剔除**：FTS5 的 unicode61 会把中文按单字分词，
    // 于是「爆」能命中「宽带噪声」（因为「爆」是「爆炸」的一个词元），
    // 造成明显无关的结果。真正有用的是二元组（「爆炸」「沉闷」）。
    // 单字只在**整串查询本身就是一个字**时才保留（AudioSet 有「鼓」这类标签）。
    let keywords: Vec<String> = {
        let all = crate::text::tokenize(&q.text);
        let whole = q.text.trim();
        let mut kept: Vec<String> = all.iter().filter(|k| !is_single_cjk(k)).cloned().collect();
        if kept.is_empty() {
            // 整串就是一个汉字（或只剩单字），此时才用单字
            kept = all;
        } else if !whole.is_empty() && !all.iter().any(|k| k == whole) {
            // 补上整串本身，让多字展示名能精确命中
            kept.push(whole.to_string());
        }
        kept
    };

    // 词汇展开后的检索：候选来自「原查询 + 各展开词」的**并集**，
    // 因此统一按 OR 求并，再按「命中了多少个不同关键词」排序。
    // 若这里还用 AND，切词产生的无效片段（「闷的」「的爆」）会把结果清零。
    let expanded: Vec<String> = q
        .extra_terms
        .iter()
        .flat_map(|t| crate::text::tokenize(t))
        .collect();
    let use_ranked = !expanded.is_empty();

    let mut rank: HashMap<i64, u32> = HashMap::new();
    let candidate_ids = if keywords.is_empty() && expanded.is_empty() {
        None
    } else {
        use crate::models::KeywordMode;

        // 把「原查询 + 展开词」合成一组检索词后再判定强弱。
        //
        // 关键：弱命中兜底的判断必须基于**合并后**的结果，
        // 否则某些词会因自身只在单字层面命中而触发兜底
        // （实测「爆炸」——真实标签是规范名「沉闷」，来自展开词——
        // 单独看原查询没有强命中，于是单字兜底把「宽带噪声」也放了进来）。
        let merged_kws: Vec<String> = keywords
            .iter()
            .cloned()
            .chain(expanded.iter().cloned())
            .collect();

        let mut ids: HashMap<i64, u32> = HashMap::new();
        let combined = keyword_match_ids_ranked(conn, &merged_kws, q.mode, 100_000)?;
        for (id, n) in &combined.strong {
            let slot = ids.entry(*id).or_insert(0);
            *slot = (*slot).max(*n);
        }
        if combined.strong.is_empty() {
            for id in combined.weak.iter() {
                ids.entry(*id).or_insert(1);
            }
        }

        // 展开词单独再查一次：原查询的 AND 语义可能把展开词的命中挡掉，
        // 而展开词代表的是同一意图的另一种说法，不应被 AND 连带排除。
        if !expanded.is_empty() {
            let extra = keyword_match_ids_ranked(conn, &expanded, KeywordMode::Or, 100_000)?;
            for (id, n) in &extra.strong {
                let slot = ids.entry(*id).or_insert(0);
                *slot = (*slot).max(*n);
            }
        }
        rank = ids.clone();
        Some(ids.into_keys().collect::<Vec<i64>>())
    };

    let (mut clauses, args) = build_filters(q, candidate_ids.as_deref());

    // 2. 必须包含的标签（每个标签一个 EXISTS，即 AND 语义）。
    //    子查询别名必须互不相同，因此带上序号。
    for (i, _tag) in q.tags.iter().enumerate() {
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM tags tq{i} WHERE tq{i}.audio_id = a.id AND tq{i}.tag = ?)"
        ));
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };

    // 标签参数在时长/格式参数之后追加，顺序必须与 SQL 中出现顺序一致。
    let mut all_args = args;
    for tag in &q.tags {
        all_args.push(Box::new(tag.clone()));
    }

    // 3. 总数
    let count_sql = format!("SELECT count(*) FROM audio_files a {where_sql}");
    let total: i64 = conn.query_row(
        &count_sql,
        params_from_iter(all_args.iter().map(|b| b.as_ref())),
        |r| r.get(0),
    )?;

    // 4. 分页取数据
    let limit = if q.limit == 0 { 500 } else { q.limit.min(5000) };
    let list_sql = format!(
        "SELECT {AUDIO_COLUMNS} FROM audio_files a {where_sql} {} LIMIT ? OFFSET ?",
        order_clause(q.sort_by, q.descending)
    );
    let mut list_args = all_args;
    list_args.push(Box::new(limit as i64));
    list_args.push(Box::new(q.offset as i64));

    let mut stmt = conn.prepare(&list_sql)?;
    let audios: Vec<AudioFile> = stmt
        .query_map(
            params_from_iter(list_args.iter().map(|b| b.as_ref())),
            map_audio_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let items = hydrate(conn, audios)?;

    // 词汇展开路径：命中关键词更多的排前面（相关性），其余保持 SQL 排序。
    let mut items = items;
    if use_ranked {
        items.sort_by(|a, b| {
            let ra = rank.get(&a.audio.id).copied().unwrap_or(0);
            let rb = rank.get(&b.audio.id).copied().unwrap_or(0);
            rb.cmp(&ra)
        });
    }

    Ok((items, total.max(0) as u32))
}

/// 为一批音频补齐标签与备注。
pub fn hydrate(conn: &Connection, audios: Vec<AudioFile>) -> Result<Vec<AudioItem>> {
    if audios.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = audios.iter().map(|a| a.id).collect();

    let mut tags_map: HashMap<i64, Vec<crate::models::Tag>> =
        tags_repo::list_for_audios(conn, &ids)?;
    let notes_map = notes_repo::notes_for_audios(conn, &ids)?;

    Ok(audios
        .into_iter()
        .map(|a| {
            let exists = std::path::Path::new(&a.path).exists();
            AudioItem {
                tags: tags_map.remove(&a.id).unwrap_or_default(),
                note: notes_map.get(&a.id).cloned().unwrap_or_default(),
                exists,
                score: None,
                audio: a,
            }
        })
        .collect())
}

/// 按 id 列表取完整条目（保持传入顺序）。
pub fn get_items(conn: &Connection, ids: &[i64]) -> Result<Vec<AudioItem>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; ids.len()].join(",");
    let sql = format!("SELECT {AUDIO_COLUMNS} FROM audio_files a WHERE a.id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let audios: Vec<AudioFile> = stmt
        .query_map(params_from_iter(ids.iter()), map_audio_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let mut items = hydrate(conn, audios)?;
    // 恢复调用方期望的顺序
    let mut by_id: HashMap<i64, AudioItem> = items.drain(..).map(|i| (i.audio.id, i)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// 库内音频总数。
pub fn count_all(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT count(*) FROM audio_files", [], |r| r.get(0))?)
}

/// 全部音频 id（切换词汇表时用于重写自动备注）。
pub fn ids_with_audio(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM audio_files ORDER BY id ASC")?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 列出所有出现过的文件夹及其文件数。
pub fn list_folders(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT folder, count(*) FROM audio_files WHERE folder IS NOT NULL AND folder <> ''
         GROUP BY folder ORDER BY count(*) DESC, folder ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 列出库内出现过的格式及数量。
pub fn list_formats(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT format, count(*) FROM audio_files WHERE format IS NOT NULL AND format <> ''
         GROUP BY format ORDER BY count(*) DESC, format ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 取所有已算出向量的 (audio_id, embedding)，供语义检索全量比对。
pub fn all_with_embeddings(conn: &Connection) -> Result<Vec<(i64, Vec<f32>)>> {
    crate::db::vector_repo::all_embeddings(conn)
}

/// 需要（重新）后台分析的音频 id：尚无任何自动标签的记录。
pub fn ids_needing_analysis(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT a.id FROM audio_files a
         WHERE NOT EXISTS (
             SELECT 1 FROM tags t
             WHERE t.audio_id = a.id AND t.source IN ('rule','yamnet','inherited','clap')
         )
         ORDER BY a.id ASC",
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
    use crate::db::open_in_memory;
    use crate::models::KeywordMode;

    fn seed(conn: &Connection) -> i64 {
        let id = insert(
            conn,
            &NewAudio {
                path: "C:/sfx/gunshot_01.wav".into(),
                filename: "gunshot_01.wav".into(),
                folder: "C:/sfx".into(),
                duration: Some(0.42),
                sample_rate: Some(48_000),
                channels: Some(2),
                format: Some("wav".into()),
                bitrate: Some(1_536_000),
                file_size: Some(40_000),
                file_hash: Some("hash-gun".into()),
            },
        )
        .unwrap();
        tags_repo::add_tag(conn, id, "枪声", crate::models::TagSource::User, 1.0).unwrap();
        notes_repo::set_note(conn, id, "短促尖锐的枪声").unwrap();
        id
    }

    #[test]
    fn insert_and_fetch_roundtrip() {
        let conn = open_in_memory().unwrap();
        let id = seed(&conn);
        let a = get(&conn, id).unwrap();
        assert_eq!(a.filename, "gunshot_01.wav");
        assert_eq!(a.format.as_deref(), Some("wav"));
        assert!(find_by_path(&conn, "C:/sfx/gunshot_01.wav")
            .unwrap()
            .is_some());
        assert!(find_by_hash(&conn, "hash-gun").unwrap().is_some());
        assert!(find_by_path(&conn, "nope.wav").unwrap().is_none());
    }

    #[test]
    fn keyword_search_hits_filename_tag_and_note() {
        let conn = open_in_memory().unwrap();
        seed(&conn);

        // 文件名（ASCII，走 FTS）
        let q = SearchQuery {
            text: "gunshot".into(),
            ..Default::default()
        };
        let (items, total) = query(&conn, &q).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].audio.filename, "gunshot_01.wav");

        // 中文标签（走 LIKE 兜底）
        let q = SearchQuery {
            text: "枪声".into(),
            ..Default::default()
        };
        let (items, _) = query(&conn, &q).unwrap();
        assert_eq!(items.len(), 1);

        // 中文备注
        let q = SearchQuery {
            text: "尖锐".into(),
            ..Default::default()
        };
        let (items, _) = query(&conn, &q).unwrap();
        assert_eq!(items.len(), 1);

        // 无命中
        let q = SearchQuery {
            text: "explosion".into(),
            ..Default::default()
        };
        let (_, total) = query(&conn, &q).unwrap();
        assert_eq!(total, 0);
    }

    /// 打标签并同步 FTS。
    ///
    /// `tags_repo::add_tag` 自身会同步，但**造完音频后必须再同步一次**
    /// （插入音频时标签还不存在）。忘了这一步就会出现「tags 表里有、
    /// FTS 索引里没有」的假失败 —— 我第一次写这些测试时就是这么踩的。
    fn tag(conn: &Connection, id: i64, name: &str, source: crate::models::TagSource) {
        crate::db::tags_repo::add_tag(conn, id, name, source, 0.9).unwrap();
        sync_fts(conn, id).unwrap();
    }

    /// 造一个只有路径/文件名不同的音频，供中文检索测试复用。
    fn mk(conn: &Connection, name: &str) -> i64 {
        insert(
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

    /// 中文没有空格时也要能搜到：整串「沉闷的爆炸」应拆出「沉闷」「爆炸」。
    ///
    /// 这是真实用户踩过的坑：库里同时有「沉闷」和「爆炸」标签，
    /// 但输入「沉闷的爆炸」搜不到任何东西 —— 因为整串被当成一个词，
    /// FTS5 短语匹配与 `LIKE %沉闷的爆炸%` 都命中不了。
    #[test]
    fn cjk_phrase_without_spaces_finds_multi_char_tags() {
        let conn = open_in_memory().unwrap();
        let a = mk(&conn, "impact.wav");
        tag(&conn, a, "沉闷", crate::models::TagSource::Rule);
        tag(&conn, a, "爆炸", crate::models::TagSource::User);

        // 用户原样输入（无空格）
        let q = SearchQuery {
            text: "沉闷的爆炸".into(),
            mode: KeywordMode::And,
            ..Default::default()
        };
        // 切词后每个片段单独能命中
        for text in ["沉闷", "爆炸"] {
            let q1 = SearchQuery {
                text: text.into(),
                mode: KeywordMode::And,
                ..Default::default()
            };
            let (it, _) = query(&conn, &q1).unwrap();
            assert_eq!(it.len(), 1, "「{text}」应单独命中");
        }
        // 原始查询（无词汇表参与）就能命中：切词产生「沉闷」「爆炸」，
        // 而 AND 交集为空时会退回 OR，因此返回两条候选；
        // 同时命中「沉闷」+「爆炸」的那条应排在**第一位**（命中数更多）。
        let (items, total) = query(&conn, &q).unwrap();
        assert!(total >= 1, "「沉闷的爆炸」不应零结果");
        assert_eq!(
            items[0].audio.filename, "impact.wav",
            "同时命中两个标签的应排在最前"
        );

        // 再加一条只带「沉闷」的音频：它也应出现，但排在 impact 之后
        let b = mk(&conn, "other.wav");
        tag(&conn, b, "沉闷", crate::models::TagSource::Rule);

        let (items, _) = query(&conn, &q).unwrap();
        assert_eq!(items.len(), 2, "两条都应出现");
        assert_eq!(
            items[0].audio.filename, "impact.wav",
            "命中 2 个关键词的应排在命中 1 个的前面"
        );
        assert_eq!(items[1].audio.filename, "other.wav");

        // OR 模式则两个都能出来
        let q_or = SearchQuery {
            mode: KeywordMode::Or,
            ..q.clone()
        };
        let (items, _) = query(&conn, &q_or).unwrap();
        assert_eq!(items.len(), 2, "OR 模式应同时命中两个音频");
    }

    /// 单个汉字也要能搜到（AudioSet 类别里有「鼓」「枪」这类单字标签）。
    #[test]
    fn single_cjk_char_is_searchable() {
        let conn = open_in_memory().unwrap();
        let id = mk(&conn, "drum.wav");
        tag(&conn, id, "鼓", crate::models::TagSource::Yamnet);

        let q = SearchQuery {
            text: "鼓".into(),
            ..Default::default()
        };
        let (items, _) = query(&conn, &q).unwrap();
        assert_eq!(items.len(), 1, "单字标签应可被单字查询命中");
    }

    /// 用户只看到展示名（如「爆炸/轰鸣」）而库里存的是规范名（「沉闷」）时，
    /// 搜索必须仍然命中 —— 这正是「词汇表只有外表不同就没意义」的那个点。
    ///
    /// 这里复刻命令层的做法：先经词汇表展开，再把展开词与原查询一起检索。
    #[test]
    fn display_label_from_vocabulary_maps_back_to_canonical_tag() {
        use crate::tag_vocab::{Resolver, TagVocab};
        let conn = open_in_memory().unwrap();
        let id = mk(&conn, "boom.wav");
        tag(&conn, id, "沉闷", crate::models::TagSource::Rule);

        let resolver = Resolver::new(TagVocab::Game, std::collections::HashMap::new());
        // 游戏词汇表下「沉闷」显示为「爆炸/轰鸣」
        assert_eq!(resolver.label("沉闷"), "爆炸/轰鸣");

        for text in ["爆炸", "轰鸣", "爆炸/轰鸣", "沉闷", "沉闷的爆炸"] {
            let extra = resolver.expand_query(text);
            let mut hit = false;
            // 原查询
            let q = SearchQuery {
                text: text.into(),
                mode: KeywordMode::Or,
                ..Default::default()
            };
            if !query(&conn, &q).unwrap().0.is_empty() {
                hit = true;
            }
            // 展开词
            for term in &extra {
                let q2 = SearchQuery {
                    text: term.clone(),
                    mode: KeywordMode::Or,
                    ..Default::default()
                };
                if !query(&conn, &q2).unwrap().0.is_empty() {
                    hit = true;
                }
            }
            assert!(
                hit,
                "「{text}」应能命中带「沉闷」标签的音频（展开词 {extra:?}）"
            );
        }
    }

    #[test]
    fn multi_keyword_and_or_semantics() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let id2 = insert(
            &conn,
            &NewAudio {
                path: "C:/sfx/explosion_03.wav".into(),
                filename: "explosion_03.wav".into(),
                folder: "C:/sfx".into(),
                duration: Some(3.0),
                sample_rate: Some(44_100),
                channels: Some(1),
                format: Some("mp3".into()),
                bitrate: None,
                file_size: None,
                file_hash: Some("hash-exp".into()),
            },
        )
        .unwrap();
        assert!(id2 > 0);

        let and_q = SearchQuery {
            text: "gunshot explosion".into(),
            mode: KeywordMode::And,
            ..Default::default()
        };
        // AND 的交集为空（没有音频同时含这两个词）时会**退回 OR 并按命中数排序**，
        // 而不是返回空列表。理由见 `keyword_match_ids_ranked` 的说明：
        // 中文切词必然产生不存在的片段，严格 AND 对中文完全不可用。
        // 两个候选各命中 1 个词，故都返回。
        let (and_items, and_total) = query(&conn, &and_q).unwrap();
        assert_eq!(and_total, 2, "AND 交集为空时应退回 OR，而不是空列表");
        assert_eq!(
            and_items
                .iter()
                .map(|i| i.audio.filename.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            2
        );

        let or_q = SearchQuery {
            text: "gunshot explosion".into(),
            mode: KeywordMode::Or,
            ..Default::default()
        };
        assert_eq!(query(&conn, &or_q).unwrap().1, 2, "OR 应命中两个");
    }

    #[test]
    fn fts_expression_escapes_user_input() {
        let expr = build_match_expr(&["a\"b".to_string(), "NEAR".to_string()], KeywordMode::And);
        assert_eq!(expr, "\"a\"\"b\" AND \"NEAR\"");
    }

    #[test]
    fn filters_by_duration_format_and_tag() {
        let conn = open_in_memory().unwrap();
        seed(&conn); // 0.42s wav

        let q = SearchQuery {
            duration: DurationBucket::Lt1,
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 1);

        let q = SearchQuery {
            duration: DurationBucket::Gt5,
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 0);

        let q = SearchQuery {
            formats: vec!["wav".into()],
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 1);

        let q = SearchQuery {
            formats: vec!["flac".into()],
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 0);

        let q = SearchQuery {
            tags: vec!["枪声".into()],
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 1);

        let q = SearchQuery {
            tags: vec!["爆炸".into()],
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 0);

        let q = SearchQuery {
            sources: vec!["user".into()],
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 1);
    }

    #[test]
    fn tags_are_hydrated_into_items() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let q = SearchQuery::default();
        let (items, total) = query(&conn, &q).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].tags.len(), 1);
        assert_eq!(items[0].tags[0].tag, "枪声");
        assert_eq!(items[0].note, "短促尖锐的枪声");
    }

    #[test]
    fn waveform_roundtrip_and_delete_cascade() {
        let conn = open_in_memory().unwrap();
        let id = seed(&conn);
        save_waveform(&conn, id, &[0, 128, 255]).unwrap();
        assert_eq!(get_waveform(&conn, id).unwrap(), vec![0, 128, 255]);

        delete(&conn, id).unwrap();
        assert_eq!(count_all(&conn).unwrap(), 0);
        // FTS 行应已清理，避免残留命中
        let q = SearchQuery {
            text: "gunshot".into(),
            ..Default::default()
        };
        assert_eq!(query(&conn, &q).unwrap().1, 0);
        let fts_rows: i64 = conn
            .query_row("SELECT count(*) FROM audio_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts_rows, 0);
    }

    #[test]
    fn sorting_by_duration_puts_nulls_last() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        insert(
            &conn,
            &NewAudio {
                path: "C:/sfx/unknown.wav".into(),
                filename: "unknown.wav".into(),
                folder: "C:/sfx".into(),
                duration: None,
                sample_rate: None,
                channels: None,
                format: Some("wav".into()),
                bitrate: None,
                file_size: None,
                file_hash: None,
            },
        )
        .unwrap();
        let q = SearchQuery {
            sort_by: SortBy::Duration,
            descending: false,
            ..Default::default()
        };
        let (items, _) = query(&conn, &q).unwrap();
        assert_eq!(items[0].audio.filename, "gunshot_01.wav");
        assert_eq!(items[1].audio.filename, "unknown.wav");
    }

    #[test]
    fn ids_needing_analysis_excludes_tagged_audio() {
        let conn = open_in_memory().unwrap();
        let a = seed(&conn); // 已有用户标签，但自动标签为空
        let b = insert(
            &conn,
            &NewAudio {
                path: "C:/sfx/b.wav".into(),
                filename: "b.wav".into(),
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
        .unwrap();
        tags_repo::add_tag(&conn, b, "低频", crate::models::TagSource::Rule, 0.8).unwrap();

        let need = ids_needing_analysis(&conn).unwrap();
        assert_eq!(need, vec![a], "已生成自动标签的音频不应再入队");
    }
}
