//! 音频相关命令：导入、导入文件夹、列表、波形、播放控制。

use crate::commands::state::AppState;
use crate::db::{self, audio_repo, is_supported_audio};
use crate::error::{AppError, Result};
use crate::indexer::queue;
use crate::models::{AudioItem, ImportReport, IndexStatus, PlaybackStateDto};
use crate::utils::config::Settings;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

/// 单次导入允许的最大文件数，防止误拖整个磁盘。
const MAX_IMPORT_FILES: usize = 20_000;

/// 展开路径：文件直接保留，目录递归收集受支持的音频。
///
/// 返回 `(待导入文件列表, 被忽略的条目数)`。
fn expand_paths(inputs: &[String]) -> (Vec<PathBuf>, u32) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut ignored = 0u32;

    for raw in inputs {
        let p = PathBuf::from(raw);
        if p.is_file() {
            if is_supported_audio(&p) {
                files.push(p);
            } else {
                ignored += 1;
            }
        } else if p.is_dir() {
            for entry in walkdir::WalkDir::new(&p)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() && is_supported_audio(entry.path()) {
                    files.push(entry.path().to_path_buf());
                    if files.len() >= MAX_IMPORT_FILES {
                        break;
                    }
                }
            }
        } else {
            ignored += 1;
        }
    }

    files.sort();
    files.dedup();
    (files, ignored)
}

/// 导入结果：报告 + 新入队的 id。
#[derive(Debug, Clone, Serialize)]
pub struct ImportOutcome {
    /// 统计报告。
    #[serde(flatten)]
    pub report: ImportReport,
    /// 本次新导入并已入队分析的音频 id。
    pub new_ids: Vec<i64>,
}

/// 导入给定的文件 / 文件夹路径（拖拽与「选择文件」都走这里）。
///
/// 重活（解码、特征提取）不在此处做：这里只读取容器元数据并落库，
/// 随后把 id 交给后台线程做完整分析，保证 UI 不卡（策划书 2.3）。
#[tauri::command]
pub async fn import_paths(state: State<'_, AppState>, paths: Vec<String>) -> Result<ImportOutcome> {
    let (files, mut ignored) = expand_paths(&paths);
    let mut report = ImportReport::default();
    let mut pending: Vec<audio_repo::NewAudio> = Vec::new();
    // 同一批内的重复文件（相同内容、不同路径）也要去重
    let mut seen_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();

    for file in &files {
        let path_str = file.to_string_lossy().to_string();

        // 1. 路径去重
        let exists = state.with_conn(|c| audio_repo::find_by_path(c, &path_str))?;
        if exists.is_some() {
            report.skipped += 1;
            report.skipped_files.push(path_str);
            continue;
        }

        // 2. 内容哈希去重（只读前 256KB，代价很低）
        let hash = crate::audio::decoder::quick_hash(file).ok();
        if let Some(h) = hash.as_deref() {
            let dup_in_db = state.with_conn(|c| audio_repo::find_by_hash(c, h))?;
            if dup_in_db.is_some() || seen_hashes.contains(h) {
                report.skipped += 1;
                report.skipped_files.push(path_str);
                continue;
            }
            seen_hashes.insert(h.to_string());
        }

        // 3. 读取元数据。失败的文件也先落库，由后台分析重试，
        //    这样用户至少能在列表里看到它（并得到失败原因）。
        let size = std::fs::metadata(file).ok().map(|m| m.len() as i64);
        let (duration, sample_rate, channels, bitrate, format) =
            match crate::audio::decoder::probe(file) {
                Ok((d, sr, ch, br, fmt)) => (
                    Some(d),
                    Some(sr as i64),
                    Some(ch as i64),
                    br.map(|b| b as i64),
                    fmt,
                ),
                Err(e) => {
                    ignored += 1;
                    report.errors.push(format!("{}: {e}", file.display()));
                    (
                        None,
                        None,
                        None,
                        None,
                        file.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase()),
                    )
                }
            };

        pending.push(audio_repo::NewAudio {
            path: path_str,
            filename: file
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            folder: file
                .parent()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            duration,
            sample_rate,
            channels,
            format,
            bitrate,
            file_size: size,
            file_hash: hash,
        });
    }

    // 4. 批量落库（单事务）
    let ids = if pending.is_empty() {
        Vec::new()
    } else {
        let n = pending.len();
        let mut conn_guard = state.conn.lock().unwrap_or_else(|e| e.into_inner());
        let ids = audio_repo::insert_many(&mut conn_guard, &pending)?;
        drop(conn_guard);
        report.imported = n as u32;
        ids
    };

    // 5. 入队后台分析
    let settings = state.with_conn(Settings::load)?;
    if settings.auto_analyze && !ids.is_empty() {
        state.with_conn_mut(|c| queue::enqueue_many(c, &ids))?;
        state.worker.notify();
    }

    // 6. 记录上次导入目录，方便下次打开
    if let Some(first) = files.first() {
        if let Some(parent) = first.parent() {
            let mut s = settings;
            s.last_import_dir = parent.to_string_lossy().to_string();
            let _ = state.with_conn(|c| s.save(c));
        }
    }

    if ignored > 0 {
        report.failed = ignored;
    }

    Ok(ImportOutcome {
        report,
        new_ids: ids,
    })
}

/// 用系统对话框选择文件夹并导入。
///
/// 返回 `None` 表示用户取消。
#[tauri::command]
pub async fn import_folder(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ImportOutcome>> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app.dialog().file().blocking_pick_folder();
    let Some(folder) = picked else {
        return Ok(None);
    };
    let path = folder
        .into_path()
        .map_err(|e| AppError::Invalid(format!("无法解析所选目录: {e}")))?;
    let outcome = import_paths(state, vec![path.to_string_lossy().to_string()]).await?;
    Ok(Some(outcome))
}

/// 用系统对话框选择若干音频文件并导入。
#[tauri::command]
pub async fn import_files(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<ImportOutcome>> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app
        .dialog()
        .file()
        .add_filter("音频文件", db::SUPPORTED_EXTENSIONS)
        .blocking_pick_files();
    let Some(list) = picked else {
        return Ok(None);
    };

    let mut paths = Vec::with_capacity(list.len());
    for f in list {
        if let Ok(p) = f.into_path() {
            paths.push(p.to_string_lossy().to_string());
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }
    let outcome = import_paths(state, paths).await?;
    Ok(Some(outcome))
}

/// 取音频的波形峰值（0-255，长度固定）。
#[tauri::command]
pub fn get_waveform(state: State<'_, AppState>, audio_id: i64) -> Result<Vec<u8>> {
    state.with_conn(|c| audio_repo::get_waveform(c, audio_id))
}

/// 按 id 取完整条目（含标签与备注）。
#[tauri::command]
pub fn get_audio(state: State<'_, AppState>, audio_id: i64) -> Result<AudioItem> {
    state.with_conn(|c| {
        let mut items = audio_repo::get_items(c, &[audio_id])?;
        items
            .pop()
            .ok_or_else(|| AppError::NotFound(format!("音频 #{audio_id}")))
    })
}

/// 列出全部音频。
#[tauri::command]
pub fn list_audio(state: State<'_, AppState>, limit: Option<u32>) -> Result<Vec<AudioItem>> {
    state.with_conn(|c| {
        let q = crate::models::SearchQuery {
            limit: limit.unwrap_or(1000),
            sort_by: crate::models::SortBy::ImportedAt,
            descending: true,
            ..Default::default()
        };
        Ok(audio_repo::query(c, &q)?.0)
    })
}

/// 删除若干音频记录（仅移出库，不删除磁盘文件）。
#[tauri::command]
pub fn delete_audio(state: State<'_, AppState>, ids: Vec<i64>) -> Result<u32> {
    if ids.is_empty() {
        return Ok(0);
    }
    state.with_conn_mut(|c| audio_repo::delete_many(c, &ids))
}

/// 重新分析指定音频（先清空自动标签，再入队）。
#[tauri::command]
pub fn reanalyze(state: State<'_, AppState>, ids: Vec<i64>) -> Result<u32> {
    if ids.is_empty() {
        return Ok(0);
    }
    let n = state.with_conn(|c| {
        for id in &ids {
            crate::db::tags_repo::clear_auto_tags(c, *id)?;
        }
        Ok(ids.len() as u32)
    })?;
    state.with_conn_mut(|c| queue::enqueue_many(c, &ids))?;
    state.worker.notify();
    Ok(n)
}

/// 把库内所有尚无自动标签的音频重新排队分析。
#[tauri::command]
pub fn analyze_all(state: State<'_, AppState>) -> Result<u32> {
    let ids = state.with_conn(audio_repo::ids_needing_analysis)?;
    if ids.is_empty() {
        return Ok(0);
    }
    let n = ids.len() as u32;
    state.with_conn_mut(|c| queue::enqueue_many(c, &ids))?;
    state.worker.notify();
    Ok(n)
}

/// 后台索引进度。
#[tauri::command]
pub fn index_status(state: State<'_, AppState>) -> Result<IndexStatus> {
    state.with_conn(|c| crate::indexer::worker::combined_status(c, &state.worker))
}

/// 在系统文件管理器中定位音频文件（打开所在文件夹并选中该文件）。
///
/// 文件已被移动或删除时，退化为打开它所在的文件夹；连文件夹都不存在时
/// 返回可读错误，而不是静默失败。
///
/// ## 为什么优先用 explorer 而不是插件
///
/// 先用 `tauri-plugin-opener` 的 `reveal_item_in_dir`（内部是
/// `SHOpenFolderAndSelectItems`）。本机实测它**返回成功却不弹窗口**
/// （插件在该 API 失败时的兜底 `ShellExecuteExW` 同样没有窗口）。
/// 改为直接启动 `explorer.exe /select,<path>` 后实测可用，
/// 这也是 Windows 上最通用的「定位到文件」手段；插件仍作为兜底保留。
#[tauri::command]
pub fn reveal_in_file_manager(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    audio_id: i64,
) -> Result<String> {
    let path = state.with_conn(|c| Ok(audio_repo::get(c, audio_id)?.path))?;
    let p = PathBuf::from(&path);

    if p.is_file() {
        match select_in_explorer(&p) {
            Ok(()) => return Ok(path),
            Err(e) => log::warn!("explorer /select 失败（{e}），改用插件兜底"),
        }
        if reveal_item(&app, &p).is_ok() {
            return Ok(path);
        }
    }

    // 文件不在了：逐级向上找到仍存在的目录并打开
    reveal_dir_only(&app, &p)
        .map_err(|e| AppError::NotFound(format!("文件与所在文件夹都不可用: {path}（{e}）")))?;
    Ok(path)
}

/// 用 `explorer.exe /select,<path>` 打开所在文件夹并选中文件。
///
/// 注意：explorer 即使成功也常返回非 0 退出码，因此**不检查退出码**，
/// 只要进程成功启动就视为成功。
#[cfg(windows)]
fn select_in_explorer(file: &std::path::Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    /// 不弹出控制台窗口。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", file.display()))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Io(std::io::Error::other(e.to_string())))
}

#[cfg(not(windows))]
fn select_in_explorer(_file: &std::path::Path) -> Result<()> {
    Err(AppError::Invalid("仅 Windows 支持 explorer 定位".into()))
}

/// 请求文件管理器选中该文件（插件实现，作为兜底）。
fn reveal_item(app: &tauri::AppHandle, file: &std::path::Path) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(file)
        .map_err(|e| AppError::Io(std::io::Error::other(e.to_string())))
}

/// 逐级向上找到存在的目录并打开它。
fn reveal_dir_only(app: &tauri::AppHandle, file: &std::path::Path) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;

    let mut dir = file.parent();
    while let Some(d) = dir {
        if d.is_dir() {
            return app
                .opener()
                .open_path(d.to_string_lossy().to_string(), None::<&str>)
                .map_err(|e| AppError::Io(std::io::Error::other(e.to_string())));
        }
        dir = d.parent();
    }
    Err(AppError::NotFound("找不到任何存在的上级目录".into()))
}

// ---------------------------------------------------------------------------
// 播放控制
// ---------------------------------------------------------------------------

/// 播放指定音频。
///
/// **解码放在阻塞线程池里**：素材可能有几十秒甚至几分钟，在主线程上解码会
/// 让整个界面（包括「按下播放键」这一下）卡住，表现为「点击不跟手」。
#[tauri::command]
pub async fn play_audio(state: State<'_, AppState>, audio_id: i64) -> Result<PlaybackStateDto> {
    let path = state.with_conn(|c| Ok(audio_repo::get(c, audio_id)?.path))?;
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(AppError::NotFound(format!("文件不存在: {path}")));
    }

    // 解码离线完成，不占用 UI 线程
    let decoded = tauri::async_runtime::spawn_blocking(move || {
        crate::audio::decoder::decode(&p, crate::audio::player::MAX_PLAY_SECONDS as f64)
    })
    .await
    .map_err(|e| AppError::Audio(format!("解码任务失败: {e}")))??;

    state.with_player(|pl| {
        pl.load_samples(
            decoded.samples,
            decoded.sample_rate,
            1,
            audio_id,
            decoded.duration,
        )?;
        Ok(pl.state())
    })
}

/// 暂停播放。
#[tauri::command]
pub fn pause_audio(state: State<'_, AppState>) -> Result<PlaybackStateDto> {
    state.with_player(|pl| {
        pl.pause();
        Ok(pl.state())
    })
}

/// 从暂停处继续。
#[tauri::command]
pub fn resume_audio(state: State<'_, AppState>) -> Result<PlaybackStateDto> {
    state.with_player(|pl| {
        pl.resume();
        Ok(pl.state())
    })
}

/// 停止播放。
#[tauri::command]
pub fn stop_audio(state: State<'_, AppState>) -> Result<PlaybackStateDto> {
    state.with_player(|pl| {
        pl.stop();
        Ok(pl.state())
    })
}

/// 跳转到指定秒数。
#[tauri::command]
pub fn seek_audio(state: State<'_, AppState>, seconds: f64) -> Result<PlaybackStateDto> {
    state.with_player(|pl| {
        pl.seek(seconds)?;
        Ok(pl.state())
    })
}

/// 设置音量（0.0 - 1.0）。
#[tauri::command]
pub fn set_volume(state: State<'_, AppState>, volume: f32) -> Result<PlaybackStateDto> {
    state.with_player(|pl| {
        pl.set_volume(volume);
        Ok(pl.state())
    })
}

/// 查询当前播放状态（前端定时轮询以更新进度条）。
#[tauri::command]
pub fn playback_state(state: State<'_, AppState>) -> Result<PlaybackStateDto> {
    let mut guard = state.player.lock().unwrap_or_else(|e| e.into_inner());
    Ok(match guard.as_mut() {
        Some(p) => p.state(),
        None => PlaybackStateDto::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_wav(path: &Path, rate: u32, secs: f32, freq: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(rate as f32 * secs) as u32 {
            let v = (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * 0.9;
            w.write_sample((v * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_imp_{}_{}", std::process::id(), tag));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn expand_paths_finds_audio_recursively() {
        let dir = temp_dir("expand");
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        write_wav(&dir.join("a.wav"), 8_000, 0.05, 440.0);
        write_wav(&sub.join("b.wav"), 8_000, 0.05, 440.0);
        std::fs::write(dir.join("readme.txt"), b"ignore me").unwrap();

        let (files, ignored) = expand_paths(&[dir.to_string_lossy().to_string()]);
        assert_eq!(files.len(), 2, "应递归找到两个 wav: {files:?}");
        assert_eq!(ignored, 0, "目录里的非音频文件不算失败");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_paths_ignores_non_audio_files_and_missing_paths() {
        let dir = temp_dir("ignore");
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, b"nope").unwrap();

        let (files, ignored) = expand_paths(&[
            txt.to_string_lossy().to_string(),
            "C:/definitely/missing/path.wav".to_string(),
        ]);
        assert!(files.is_empty());
        assert_eq!(ignored, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_paths_deduplicates_same_file_given_twice() {
        let dir = temp_dir("dedup");
        let f = dir.join("a.wav");
        write_wav(&f, 8_000, 0.05, 440.0);
        let s = f.to_string_lossy().to_string();
        let (files, _) = expand_paths(&[s.clone(), s]);
        assert_eq!(files.len(), 1, "同一文件重复传入只应保留一次");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_paths_accepts_all_documented_formats() {
        let dir = temp_dir("formats");
        for ext in ["wav", "mp3", "ogg", "flac", "m4a", "aiff"] {
            std::fs::write(dir.join(format!("x.{ext}")), b"stub").unwrap();
        }
        let (files, _) = expand_paths(&[dir.to_string_lossy().to_string()]);
        assert_eq!(files.len(), 6, "策划书列出的 6 种格式都应被接受: {files:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
