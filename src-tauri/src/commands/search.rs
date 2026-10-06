//! 搜索命令：关键词检索（FTS5 + LIKE 兜底）与语义检索。

use crate::commands::state::AppState;
use crate::db::audio_repo;
use crate::error::Result;
use crate::ml::clap_text;
use crate::models::{AudioItem, SearchQuery, SearchResult, SortBy};
use tauri::State;

/// 语义检索的**置信度门槛**。
///
/// 背景：`Xenova/clap-htsat-unfused` 的 ONNX 导出跨模态对齐并不稳定 ——
/// 实测语义簇之间距离大的查询表现很好（「a pop song...」对音乐 +0.480，
/// 对提示音只有 +0.078），但相近语义簇之间会混淆（「a bell chiming」最像
/// 却是 +0.178 的弱匹配）。
///
/// 与其把弱匹配当结果端给用户，不如设门槛：低于它就说明「模型没把握」，
/// 此时明确告知并退回关键词检索。这不是掩盖问题，而是避免让用户
/// 在不知情的情况下得到一堆看似相关、实则噪声的结果。
///
/// 取值依据：上例中真实匹配 >= 0.26，明显错误匹配 <= 0.18，故取 0.20。
pub const SEMANTIC_MIN_SCORE: f32 = 0.20;

/// 关键词 + 筛选 + 排序检索。
///
/// 走 FTS5；中文等 CJK 查询在 `unicode61` 分词下无法被 `MATCH` 命中，
/// 因此仓储层同时做了 LIKE 兜底（见 [`audio_repo::keyword_match_ids`]）。
///
/// 查询串会先经过**词汇表展开**：用户在「游戏」词汇表下搜「激光」时，
/// 会自动补上规范标签「尖锐」，从而命中所有同类音效。
#[tauri::command]
pub fn search_audio(state: State<'_, AppState>, query: SearchQuery) -> Result<SearchResult> {
    state.with_conn(|c| {
        let resolver = crate::utils::config::Settings::load(c)
            .map(|s| s.tag_resolver())
            .unwrap_or_else(|_| {
                crate::tag_vocab::Resolver::new(Default::default(), Default::default())
            });

        // 词汇展开出的规范标签（例如「爆炸」->「沉闷」、「激光」->「尖锐」）
        let extra_terms = resolver.expand_query(&query.text);

        if extra_terms.is_empty() {
            let (items, total) = audio_repo::query(c, &query)?;
            return Ok(SearchResult {
                items,
                total,
                mode: "keyword".into(),
                notice: None,
            });
        }

        // 交给仓储层：它会取「原查询 ∪ 展开词」的并集，
        // 并按命中的不同关键词数量排序（命中「沉闷」+「爆炸」的排在只命中一个的前面）。
        let mut q = query.clone();
        q.extra_terms = extra_terms.clone();
        let (items, total) = audio_repo::query(c, &q)?;

        Ok(SearchResult {
            items,
            total,
            mode: "keyword".into(),
            notice: Some(format!(
                "已按当前词汇表把「{}」关联到规范标签「{}」，两者一起检索。",
                query.text,
                extra_terms.join("、")
            )),
        })
    })
}

/// 语义检索（自然语言）。
///
/// **当前行为**：CLAP 模型按用户决定暂未下载，[`clap_text::encode_if_available`]
/// 返回 `None`，于是自动**降级为关键词搜索**，并在 `notice` 中说明原因与原查询。
/// 这样「自然语言搜索」入口从第一天起就是可用的，只是精度受限于关键词匹配；
/// 模型接入后本函数无需改动即可切换到真正的向量检索。
#[tauri::command]
pub fn semantic_search(
    state: State<'_, AppState>,
    text: String,
    limit: Option<u32>,
) -> Result<SearchResult> {
    let normalized = clap_text::normalize_query(&text);
    let limit = limit.unwrap_or(50).clamp(1, 500);

    let encoded = state.with_conn(|c| clap_text::encode_if_available(c, &normalized))?;

    match encoded {
        Some(query_vec) => {
            // 文本向量可用，但还需要音频向量才能比对。
            // 若音频向量为空（音频塔尚未接入 / 还没建索引），直接返回空列表会让
            // 用户看到「搜不到任何东西」却不知原因，因此这里明确降级并说明。
            state.with_conn(|c| {
                let candidates = audio_repo::all_with_embeddings(c)?;
                if candidates.is_empty() {
                    let q = SearchQuery {
                        text: normalized.clone(),
                        mode: crate::models::KeywordMode::Or,
                        sort_by: SortBy::ImportedAt,
                        descending: true,
                        limit,
                        ..Default::default()
                    };
                    let (items, total) = audio_repo::query(c, &q)?;
                    return Ok(SearchResult {
                        items,
                        total,
                        mode: "keyword".into(),
                        notice: Some(
                            "文本编码器已就绪，但库内尚无音频向量，暂时按关键词检索。\
                             音频向量需要 CLAP 音频编码器完成索引后才会生成。"
                                .into(),
                        ),
                    });
                }

                // 模型可用：全量余弦比对，取 Top-K（阈值 0，先看最高分再决定是否采信）
                let hits =
                    crate::ml::similarity::top_k(&query_vec, &candidates, limit as usize, 0.0);
                let best = hits.first().map(|(_, s)| *s).unwrap_or(0.0);

                // 置信度不足：明确告知并按关键词检索，而不是端出弱匹配
                if best < SEMANTIC_MIN_SCORE {
                    let mut q = SearchQuery {
                        text: normalized.clone(),
                        mode: crate::models::KeywordMode::Or,
                        sort_by: SortBy::ImportedAt,
                        descending: true,
                        limit,
                        ..Default::default()
                    };
                    // 关键词检索同样要享受词汇表展开：
                    // 用户可能输入的是展示名（如「爆炸」），而库里存的是规范名（「沉闷」）。
                    q.extra_terms = crate::utils::config::Settings::load(c)
                        .map(|s| s.tag_resolver())
                        .unwrap_or_else(|_| {
                            crate::tag_vocab::Resolver::new(Default::default(), Default::default())
                        })
                        .expand_query(&normalized);
                    let (items, total) = audio_repo::query(c, &q)?;
                    return Ok(SearchResult {
                        items,
                        total,
                        mode: "keyword".into(),
                        notice: Some(format!(
                            "语义模型对这条描述把握不足（最高 {best:.2}，门槛 {SEMANTIC_MIN_SCORE:.2}），\
                             已改用关键词检索。若结果不对，可以直接搜标签里的词\
                             （例如「沉闷」「爆炸」），多个词用空格分开。"
                        )),
                    });
                }

                let ids: Vec<i64> = hits.iter().map(|(id, _)| *id).collect();
                let score_map: std::collections::HashMap<i64, f32> = hits.into_iter().collect();
                let mut items = audio_repo::get_items(c, &ids)?;
                for it in items.iter_mut() {
                    it.score = score_map.get(&it.audio.id).copied();
                }
                let total = items.len() as u32;
                Ok(SearchResult {
                    items,
                    total,
                    mode: "semantic".into(),
                    notice: None,
                })
            })
        }
        None => {
            // 模型缺失：降级为关键词搜索
            state.with_conn(|c| {
                let q = SearchQuery {
                    text: normalized.clone(),
                    mode: crate::models::KeywordMode::Or,
                    sort_by: SortBy::ImportedAt,
                    descending: true,
                    limit,
                    ..Default::default()
                };
                let (items, total) = audio_repo::query(c, &q)?;
                Ok(SearchResult {
                    items,
                    total,
                    mode: "keyword".into(),
                    notice: Some(
                        "语义模型（CLAP）尚未安装，已自动使用关键词搜索。\
                         安装模型后即可获得真正的自然语言检索：见「设置 → 模型管理」。"
                            .into(),
                    ),
                })
            })
        }
    }
}

/// 与给定音频相似的音频（「找像这个的声音」）。
///
/// 依赖音频向量；模型未接入时返回空列表并附带说明。
#[tauri::command]
pub fn similar_audio(
    state: State<'_, AppState>,
    audio_id: i64,
    limit: Option<u32>,
) -> Result<SearchResult> {
    state.with_conn(|c| {
        let target = match crate::db::vector_repo::get_embedding(c, audio_id)? {
            Some(v) => v,
            None => {
                return Ok(SearchResult {
                    items: Vec::new(),
                    total: 0,
                    mode: "similar".into(),
                    notice: Some(
                        "该音频尚无向量特征（需要 CLAP 音频编码器）。\
                         模型安装后重新分析即可启用相似音频检索。"
                            .into(),
                    ),
                })
            }
        };

        let candidates = audio_repo::all_with_embeddings(c)?;
        let hits = crate::ml::similarity::top_k(
            &target,
            &candidates,
            (limit.unwrap_or(20) as usize) + 1,
            0.0,
        );
        // 排除自身
        let hits: Vec<(i64, f32)> = hits.into_iter().filter(|(id, _)| *id != audio_id).collect();
        let ids: Vec<i64> = hits.iter().map(|(id, _)| *id).collect();
        let score_map: std::collections::HashMap<i64, f32> = hits.into_iter().collect();

        let mut items: Vec<AudioItem> = audio_repo::get_items(c, &ids)?;
        for it in items.iter_mut() {
            it.score = score_map.get(&it.audio.id).copied();
        }
        let total = items.len() as u32;
        Ok(SearchResult {
            items,
            total,
            mode: "similar".into(),
            notice: None,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{audio_repo::NewAudio, open_in_memory};

    fn seed(conn: &rusqlite::Connection) {
        for (name, dur, fmt) in [
            ("gunshot_01.wav", 0.3, "wav"),
            ("explosion_big.wav", 3.0, "wav"),
            ("ui_click.mp3", 0.05, "mp3"),
        ] {
            let id = audio_repo::insert(
                conn,
                &NewAudio {
                    path: format!("C:/sfx/{name}"),
                    filename: name.into(),
                    folder: "C:/sfx".into(),
                    duration: Some(dur),
                    sample_rate: Some(48_000),
                    channels: Some(2),
                    format: Some(fmt.into()),
                    bitrate: None,
                    file_size: None,
                    file_hash: Some(format!("hash-{name}")),
                },
            )
            .unwrap();
            let _ = id;
        }
    }

    #[test]
    fn keyword_search_matches_filename() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let q = SearchQuery {
            text: "gunshot".into(),
            ..Default::default()
        };
        let (items, total) = audio_repo::query(&conn, &q).unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].audio.filename, "gunshot_01.wav");
    }

    #[test]
    fn search_returns_all_when_query_empty() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let q = SearchQuery::default();
        let (_, total) = audio_repo::query(&conn, &q).unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn format_and_duration_filters_compose() {
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let q = SearchQuery {
            formats: vec!["wav".into()],
            duration: crate::models::DurationBucket::Gt5,
            ..Default::default()
        };
        assert_eq!(
            audio_repo::query(&conn, &q).unwrap().1,
            0,
            "wav 中没有 >5s 的"
        );

        let q = SearchQuery {
            formats: vec!["wav".into()],
            duration: crate::models::DurationBucket::B1To5,
            ..Default::default()
        };
        assert_eq!(audio_repo::query(&conn, &q).unwrap().1, 1);
    }

    #[test]
    fn semantic_fallback_normalizes_query() {
        // 降级路径会先规整查询串；这里验证规整后的关键词仍能命中
        let conn = open_in_memory().unwrap();
        seed(&conn);
        let normalized = clap_text::normalize_query("  gunshot   explosion ");
        assert_eq!(normalized, "gunshot explosion");

        let q = SearchQuery {
            text: normalized,
            mode: crate::models::KeywordMode::Or,
            ..Default::default()
        };
        assert_eq!(audio_repo::query(&conn, &q).unwrap().1, 2);
    }
}
