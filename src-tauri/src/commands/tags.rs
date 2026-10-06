//! 标签词汇表命令：切换「游戏 / 剪辑 / 音乐 / 自定义」的说法。

use crate::commands::state::AppState;
use crate::db::{audio_repo, notes_repo, tags_repo};
use crate::error::Result;
use crate::tag_vocab::{Resolver, TagVocab};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

/// 对整个库重算相似度标签继承。
///
/// 用户给一批变体里的某一个打了标签后，手动触发一次即可让其余变体跟上。
/// 也适合在批量导入完成后调用 —— 此时一个音效簇已经全部入库，
/// 继承才能找到足够相似的邻居。
#[tauri::command]
pub fn inherit_tags(state: State<'_, AppState>) -> Result<InheritReport> {
    state.with_conn(|c| {
        let (processed, inherited, pruned) = crate::ml::inherit::inherit_all(c)?;
        // inherit_for 内部已按音频同步 FTS，这里无需再刷
        Ok(InheritReport {
            processed,
            inherited,
            pruned,
        })
    })
}

/// 继承结果，用于界面提示。
#[derive(Debug, Serialize)]
pub struct InheritReport {
    /// 参与计算的音频数（只有已生成向量的才会被处理）。
    pub processed: usize,
    /// 新增 / 更新的继承标签数。
    pub inherited: u32,
    /// 清理掉的过期继承标签数。
    pub pruned: u32,
}
/// 一个词汇表的元信息 + 对照表。
#[derive(Debug, Serialize)]
pub struct VocabInfo {
    /// 标识。
    pub id: String,
    /// 展示名。
    pub label: String,
    /// 目标用户，用于设置面板说明。
    pub audience: String,
    /// 一句话说明。
    pub hint: String,
    /// 规范标签 -> 该词汇表下的说法。
    pub table: Vec<(String, String)>,
}

/// 全部词汇表 + 当前选中项 + 自定义覆盖。
#[derive(Debug, Serialize)]
pub struct VocabList {
    /// 当前词汇表标识。
    pub current: String,
    /// 自定义覆盖表（规范标签 -> 用户说法）。
    pub overrides: HashMap<String, String>,
    /// 全部词汇表。
    pub vocabularies: Vec<VocabInfo>,
}

/// 元信息（展示名 / 目标用户 / 说明）。
fn meta(v: TagVocab) -> (&'static str, &'static str, &'static str, &'static str) {
    match v {
        TagVocab::Game => (
            "game",
            "游戏创作者",
            "游戏开发者 / 技术美术",
            "用「这是什么音效」说话：沉闷 → 爆炸 / 轰鸣，尖锐 → 激光 / 枪击",
        ),
        TagVocab::Video => (
            "video",
            "剪辑 / 视频",
            "视频剪辑 / 短视频创作者",
            "用「听感与用途」说话：沉闷 → 低沉轰响，长音 → 长镜头铺底",
        ),
        TagVocab::Music => (
            "music",
            "音乐制作人",
            "音乐制作 / 音效设计",
            "用专业术语说话：沉闷 → 低频厚重，瞬态 → 瞬态（快 attack）",
        ),
        TagVocab::Custom => (
            "custom",
            "自定义",
            "任何想用自己的词的人",
            "在默认术语基础上逐项改名，例如把「沉闷」改成你自己的说法",
        ),
    }
}

/// 列出全部词汇表。
#[tauri::command]
pub fn list_tag_vocabularies(state: State<'_, AppState>) -> Result<VocabList> {
    state.with_conn(|c| {
        let settings = crate::utils::config::Settings::load(c)?;
        let vocabularies = TagVocab::all()
            .iter()
            .map(|v| {
                let (id, label, audience, hint) = meta(*v);
                // 对照表按该词汇表自己的映射生成（自定义表含用户覆盖）
                let resolver = Resolver::new(*v, settings.tag_overrides.clone());
                VocabInfo {
                    id: id.to_string(),
                    label: label.to_string(),
                    audience: audience.to_string(),
                    hint: hint.to_string(),
                    table: resolver.table(),
                }
            })
            .collect();

        Ok(VocabList {
            current: settings.tag_vocab.as_db().to_string(),
            overrides: settings.tag_overrides,
            vocabularies,
        })
    })
}

/// 切换词汇表。
///
/// 切换后**重新生成自动备注**（规则标签没变，只是换了一种说法），
/// 用户自己写的备注不受影响。
#[tauri::command]
pub fn set_tag_vocabulary(state: State<'_, AppState>, vocab: String) -> Result<u32> {
    let parsed = TagVocab::from_db(&vocab);
    let updated = state.with_conn(|c| {
        let mut s = crate::utils::config::Settings::load(c)?;
        s.tag_vocab = parsed;
        s.save(c)?;
        regenerate_auto_notes(c, &s.tag_resolver())
    })?;
    Ok(updated)
}

/// 设置自定义词汇表的重命名项（`None` 表示删除该项，恢复默认）。
#[tauri::command]
pub fn set_tag_override(
    state: State<'_, AppState>,
    canonical: String,
    label: Option<String>,
) -> Result<u32> {
    state.with_conn(|c| {
        let mut s = crate::utils::config::Settings::load(c)?;
        match label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
        {
            Some(v) => {
                s.tag_overrides.insert(canonical, v);
            }
            None => {
                s.tag_overrides.remove(&canonical);
            }
        }
        s.save(c)?;
        regenerate_auto_notes(c, &s.tag_resolver())
    })
}

/// 把所有「自动生成的备注」按当前词汇表重写一遍。
///
/// 只重写那些**看起来是自动备注**的记录（以「的音频」结尾或以「未分类音频」开头），
/// 用户自己写的备注一律不动 —— 我们无法可靠区分两者，因此用保守的启发式。
/// 返回被更新的条数。
fn regenerate_auto_notes(conn: &rusqlite::Connection, resolver: &Resolver) -> Result<u32> {
    let ids = audio_repo::ids_with_audio(conn)?;
    let mut updated = 0u32;
    for id in ids {
        let notes = notes_repo::list(conn, id)?;
        if notes.is_empty() {
            continue;
        }
        // 取第一条作为判断依据
        let first = &notes[0].note;
        let looks_auto = first.ends_with("的音频") || first == "未分类音频";
        if !looks_auto {
            continue;
        }
        let tags = tags_repo::list(conn, id)?;
        let note = crate::audio::features::generate_note_with(&tags, &|t| resolver.label(t));
        if note != "未分类音频" {
            notes_repo::set_note(conn, id, &note)?;
            updated += 1;
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::models::TagSource;

    fn seed(conn: &rusqlite::Connection) -> i64 {
        let id = audio_repo::insert(
            conn,
            &audio_repo::NewAudio {
                path: "C:/sfx/boom.wav".into(),
                filename: "boom.wav".into(),
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
        tags_repo::add_tag(conn, id, "沉闷", TagSource::Rule, 0.85).unwrap();
        tags_repo::add_tag(conn, id, "低频", TagSource::Rule, 0.8).unwrap();
        notes_repo::set_note(conn, id, "沉闷、低频的音频").unwrap();
        id
    }

    #[test]
    fn vocab_metadata_covers_all_four() {
        let ids: Vec<&str> = TagVocab::all().iter().map(|v| meta(*v).0).collect();
        assert_eq!(ids, vec!["game", "video", "music", "custom"]);
        for v in TagVocab::all() {
            let (_, label, audience, hint) = meta(*v);
            assert!(!label.is_empty());
            assert!(!audience.is_empty());
            assert!(!hint.is_empty());
        }
    }

    #[test]
    fn resolver_table_matches_vocabulary_size() {
        let conn = open_in_memory().unwrap();
        let s = crate::utils::config::Settings::load(&conn).unwrap();
        for v in TagVocab::all() {
            let r = Resolver::new(*v, s.tag_overrides.clone());
            assert!(!r.table().is_empty(), "{:?} 的对照表不应为空", v);
        }
    }

    /// 切换词汇表后，自动备注应被重写成新说法。
    #[test]
    fn switching_vocabulary_rewrites_auto_notes_only() {
        let conn = open_in_memory().unwrap();
        let id = seed(&conn);

        // 默认（游戏）
        let s = crate::utils::config::Settings::load(&conn).unwrap();
        assert_eq!(s.tag_vocab, TagVocab::Game);
        let r = s.tag_resolver();
        assert_eq!(r.label("沉闷"), "爆炸/轰鸣");

        // 切到音乐词汇表
        let mut s2 = s.clone();
        s2.tag_vocab = TagVocab::Music;
        s2.save(&conn).unwrap();
        let updated = regenerate_auto_notes(&conn, &s2.tag_resolver()).unwrap();
        assert_eq!(updated, 1, "应更新 1 条自动备注");

        let note = notes_repo::list(&conn, id).unwrap()[0].note.clone();
        assert!(note.contains("低频厚重"), "音乐词汇表下备注应为：{note}");
        assert!(!note.contains("沉闷"), "不应再出现规范词：{note}");
    }

    /// 用户自己写的备注绝不能被重写。
    #[test]
    fn switching_vocabulary_preserves_user_notes() {
        let conn = open_in_memory().unwrap();
        let id = seed(&conn);
        notes_repo::set_note(&conn, id, "我自己写的：开场爆炸").unwrap();

        let mut s = crate::utils::config::Settings::load(&conn).unwrap();
        s.tag_vocab = TagVocab::Video;
        s.save(&conn).unwrap();
        let updated = regenerate_auto_notes(&conn, &s.tag_resolver()).unwrap();

        assert_eq!(updated, 0, "用户备注不应被重写");
        assert_eq!(
            notes_repo::list(&conn, id).unwrap()[0].note,
            "我自己写的：开场爆炸"
        );
    }

    #[test]
    fn custom_override_changes_label_and_note() {
        let conn = open_in_memory().unwrap();
        let id = seed(&conn);
        let mut s = crate::utils::config::Settings::load(&conn).unwrap();
        s.tag_vocab = TagVocab::Custom;
        s.tag_overrides
            .insert("沉闷".to_string(), "我的超重低音".to_string());
        s.save(&conn).unwrap();

        let r = s.tag_resolver();
        assert_eq!(r.label("沉闷"), "我的超重低音");
        assert_eq!(r.label("低频"), "低频", "未覆盖的项仍用规范词");

        regenerate_auto_notes(&conn, &r).unwrap();
        let note = notes_repo::list(&conn, id).unwrap()[0].note.clone();
        assert!(note.contains("我的超重低音"), "{note}");
    }

    #[test]
    fn switching_vocabulary_persists() {
        let conn = open_in_memory().unwrap();
        let mut s = crate::utils::config::Settings::load(&conn).unwrap();
        s.tag_vocab = TagVocab::Music;
        s.save(&conn).unwrap();
        assert_eq!(
            crate::utils::config::Settings::load(&conn)
                .unwrap()
                .tag_vocab,
            TagVocab::Music
        );
    }
}
