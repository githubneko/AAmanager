//! 标签与备注命令（策划书 Phase 2 的 CRUD 与批量编辑）。

use crate::commands::state::AppState;
use crate::db::{audio_repo, notes_repo, tags_repo};
use crate::error::Result;
use crate::models::{Tag, TagCount, TagSource, UserNote};
use tauri::State;

/// 列出某音频的标签。
#[tauri::command]
pub fn list_tags(state: State<'_, AppState>, audio_id: i64) -> Result<Vec<Tag>> {
    state.with_conn(|c| tags_repo::list(c, audio_id))
}

/// 列出全部标签及使用频率（标签云）。
#[tauri::command]
pub fn list_all_tags(state: State<'_, AppState>, limit: Option<u32>) -> Result<Vec<TagCount>> {
    state.with_conn(|c| tags_repo::list_all_with_counts(c, limit.unwrap_or(300)))
}

/// 标签自动补全。
#[tauri::command]
pub fn suggest_tags(
    state: State<'_, AppState>,
    prefix: String,
    limit: Option<u32>,
) -> Result<Vec<String>> {
    state.with_conn(|c| tags_repo::suggest(c, &prefix, limit.unwrap_or(20)))
}

/// 给单个音频添加标签（用户来源）。
#[tauri::command]
pub fn add_tag(state: State<'_, AppState>, audio_id: i64, tag: String) -> Result<Vec<Tag>> {
    state.with_conn(|c| {
        tags_repo::add_tag(c, audio_id, &tag, TagSource::User, 1.0)?;
        tags_repo::list(c, audio_id)
    })
}

/// 批量给多个音频添加同一标签。
#[tauri::command]
pub fn add_tag_many(state: State<'_, AppState>, ids: Vec<i64>, tag: String) -> Result<u32> {
    state.with_conn_mut(|c| tags_repo::add_tag_many(c, &ids, &tag, TagSource::User))
}

/// 删除某音频上的一个标签。
#[tauri::command]
pub fn remove_tag(state: State<'_, AppState>, audio_id: i64, tag: String) -> Result<Vec<Tag>> {
    state.with_conn(|c| {
        tags_repo::remove_tag(c, audio_id, &tag)?;
        tags_repo::list(c, audio_id)
    })
}

/// 确认自动标签（置信度提升为已确认）。
#[tauri::command]
pub fn confirm_tag(state: State<'_, AppState>, audio_id: i64, tag: String) -> Result<Vec<Tag>> {
    state.with_conn(|c| {
        tags_repo::confirm_tag(c, audio_id, &tag)?;
        tags_repo::list(c, audio_id)
    })
}

/// 全局重命名标签。
#[tauri::command]
pub fn rename_tag(state: State<'_, AppState>, from: String, to: String) -> Result<u32> {
    state.with_conn(|c| tags_repo::rename_tag(c, &from, &to))
}

// ---------------------------------------------------------------------------
// 备注
// ---------------------------------------------------------------------------

/// 列出某音频的全部备注。
#[tauri::command]
pub fn list_notes(state: State<'_, AppState>, audio_id: i64) -> Result<Vec<UserNote>> {
    state.with_conn(|c| notes_repo::list(c, audio_id))
}

/// 覆盖设置某音频的备注（传空串即清空）。
#[tauri::command]
pub fn set_note(state: State<'_, AppState>, audio_id: i64, note: String) -> Result<String> {
    state.with_conn(|c| {
        notes_repo::set_note(c, audio_id, &note)?;
        Ok(note)
    })
}

/// 批量设置备注。
#[tauri::command]
pub fn set_note_many(state: State<'_, AppState>, ids: Vec<i64>, note: String) -> Result<u32> {
    state.with_conn_mut(|c| notes_repo::set_note_many(c, &ids, &note))
}

/// 追加一条备注（不覆盖已有内容）。
#[tauri::command]
pub fn append_note(
    state: State<'_, AppState>,
    audio_id: i64,
    note: String,
) -> Result<Vec<UserNote>> {
    state.with_conn(|c| {
        notes_repo::append_note(c, audio_id, &note)?;
        notes_repo::list(c, audio_id)
    })
}

/// 删除单条备注。
#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, note_id: i64) -> Result<()> {
    state.with_conn(|c| notes_repo::delete(c, note_id))
}

/// 批量编辑：一次性替换一组音频的标签与备注（前端批量编辑面板使用）。
#[derive(Debug, serde::Deserialize)]
pub struct BulkEdit {
    /// 目标音频 id。
    pub ids: Vec<i64>,
    /// 要添加的标签（可选）。
    #[serde(default)]
    pub add_tags: Vec<String>,
    /// 要移除的标签（可选，按音频逐个删除）。
    #[serde(default)]
    pub remove_tags: Vec<String>,
    /// 覆盖备注；`None` 表示不改动备注。
    #[serde(default)]
    pub note: Option<String>,
}

/// 批量编辑结果。
#[derive(Debug, serde::Serialize)]
pub struct BulkEditResult {
    /// 受影响的音频数。
    pub affected: u32,
    /// 新增的标签关联数。
    pub tags_added: u32,
    /// 移除的标签关联数。
    pub tags_removed: u32,
    /// 备注被更新的音频数。
    pub notes_updated: u32,
}

/// 对多个音频执行批量标签 / 备注编辑。
#[tauri::command]
pub fn bulk_edit(state: State<'_, AppState>, edit: BulkEdit) -> Result<BulkEditResult> {
    if edit.ids.is_empty() {
        return Ok(BulkEditResult {
            affected: 0,
            tags_added: 0,
            tags_removed: 0,
            notes_updated: 0,
        });
    }

    let mut result = BulkEditResult {
        affected: edit.ids.len() as u32,
        tags_added: 0,
        tags_removed: 0,
        notes_updated: 0,
    };

    state.with_conn(|c| {
        for tag in &edit.add_tags {
            result.tags_added += tags_repo::add_tag_many(c, &edit.ids, tag, TagSource::User)?;
        }
        for tag in &edit.remove_tags {
            for id in &edit.ids {
                tags_repo::remove_tag(c, *id, tag)?;
                result.tags_removed += 1;
            }
        }
        Ok(())
    })?;

    if let Some(note) = &edit.note {
        result.notes_updated =
            state.with_conn(|c| notes_repo::set_note_many(c, &edit.ids, note))?;
    }

    // 备注 / 标签写入后刷新 FTS 已由仓储层完成；这里只需保证列表数据一致
    Ok(result)
}

/// 批量删除标签（从多个音频上移除同一标签）。
#[tauri::command]
pub fn remove_tag_many(state: State<'_, AppState>, ids: Vec<i64>, tag: String) -> Result<u32> {
    state.with_conn(|c| {
        let mut n = 0;
        for id in &ids {
            tags_repo::remove_tag(c, *id, &tag)?;
            n += 1;
        }
        Ok(n)
    })
}

/// 库概览统计。
#[derive(Debug, serde::Serialize)]
pub struct LibraryStats {
    /// 音频总数。
    pub total: i64,
    /// 标签种类数。
    pub tag_count: usize,
    /// 各格式数量。
    pub formats: Vec<(String, i64)>,
    /// 各文件夹数量。
    pub folders: Vec<(String, i64)>,
}

/// 取库概览统计（筛选面板用）。
#[tauri::command]
pub fn library_stats(state: State<'_, AppState>) -> Result<LibraryStats> {
    state.with_conn(|c| {
        Ok(LibraryStats {
            total: audio_repo::count_all(c)?,
            tag_count: tags_repo::list_all_with_counts(c, 0)?.len(),
            formats: audio_repo::list_formats(c)?,
            folders: audio_repo::list_folders(c)?,
        })
    })
}
