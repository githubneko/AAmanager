//! 设置与模型管理命令。

use crate::commands::state::AppState;
use crate::error::{AppError, Result};
use crate::ml::downloader::{DownloadProgress, EVENT_PROGRESS};
use crate::ml::model_manager::{ModelStatus, REGISTRY};
use crate::utils::config::{Settings, KEY_MODELS_DIR, THEMES};
use serde::Serialize;
use tauri::{Emitter, State};

/// 读取全部设置。
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    state.with_conn(Settings::load)
}

/// 写入全部设置（前端整体提交）。
///
/// 若模型目录发生变化，会同步重配 [`crate::ml::model_manager::ModelManager`]。
#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    // 目录变更前先校验可用性，避免把模型指向一个不可写的位置
    let new_dir = settings.effective_models_dir();
    if !settings.models_dir.trim().is_empty()
        && !crate::ml::model_manager::ModelManager::dir_is_usable(&new_dir)
    {
        return Err(AppError::Invalid(format!(
            "模型目录不可写: {}",
            new_dir.display()
        )));
    }

    state.with_conn(|c| settings.save(c))?;
    state.with_models(|m| {
        m.set_dir(new_dir);
        m.ensure_dir()
    })?;
    state.with_conn(Settings::load)
}

/// 可用主题列表（供前端渲染主题选择器，避免前后端清单不一致）。
#[tauri::command]
pub fn list_themes() -> Vec<&'static str> {
    THEMES.to_vec()
}

/// 模型目录与各模型状态的汇总。
#[derive(Debug, Serialize)]
pub struct ModelsInfo {
    /// 模型存放目录（提示用户可手动放置模型）。
    pub models_dir: String,
    /// 平台默认模型目录（供「恢复默认」使用）。
    pub default_models_dir: String,
    /// 各模型状态。
    pub models: Vec<ModelStatus>,
    /// 语义搜索是否可用（三个模型文件齐备）。
    pub semantic_ready: bool,
    /// 推理内核是否随程序分发（ONNX Runtime 静态链接，恒为 true）。
    ///
    /// 界面上要明确告诉用户「内核不需要你下载」——
    /// 之前这里写死了一句「推理内核尚未接入」，内核接好了那行文案却没改，
    /// 导致功能明明能用却显示未接入。
    pub kernel_bundled: bool,
    /// 当前已加载进内存的 ONNX 会话数（0 表示尚未推理过，属正常）。
    pub inference_sessions: usize,
    /// 自动标签是否可用（AST 事件分类模型就位）。
    pub tagging_ready: bool,
    /// 已生成向量的音频数。
    pub embedded_audio: i64,
    /// 正在下载的模型名列表。
    pub downloading: Vec<String>,
    /// 目录是否可写。
    pub dir_writable: bool,
}

/// 查询模型状态。
#[tauri::command]
pub fn models_info(state: State<'_, AppState>) -> Result<ModelsInfo> {
    let embedded = state.with_conn(crate::db::vector_repo::count_embeddings)?;
    state.with_models(|m| {
        let statuses = m.statuses();
        let dir = m.models_dir().to_path_buf();
        let downloading = statuses
            .iter()
            .filter(|s| state.downloads.is_downloading(&s.name))
            .map(|s| s.name.clone())
            .collect();
        Ok(ModelsInfo {
            models_dir: dir.to_string_lossy().to_string(),
            default_models_dir: crate::ml::model_manager::default_models_dir()
                .to_string_lossy()
                .to_string(),
            models: statuses,
            semantic_ready: m.semantic_ready(),
            kernel_bundled: true,
            inference_sessions: crate::ml::inference::global().loaded_count(),
            tagging_ready: crate::ml::yamnet::is_ready(m),
            embedded_audio: embedded,
            downloading,
            dir_writable: crate::ml::model_manager::ModelManager::dir_is_usable(&dir),
        })
    })
}

/// 模型下载源信息。
#[derive(Debug, Serialize)]
pub struct ModelDownloadInfo {
    /// 模型文件名。
    pub name: String,
    /// 展示名。
    pub display_name: String,
    /// 用途。
    pub purpose: String,
    /// 主下载地址。
    pub url: String,
    /// 备用镜像地址。
    pub mirror_url: Option<String>,
    /// 期望大小（字节）。
    pub size_bytes: u64,
    /// 是否为语义搜索必需。
    pub required_for_semantic: bool,
}

/// 列出可下载模型及其地址。
#[tauri::command]
pub fn model_download_list() -> Vec<ModelDownloadInfo> {
    REGISTRY
        .iter()
        .map(|m| ModelDownloadInfo {
            name: m.name.to_string(),
            display_name: m.display_name.to_string(),
            purpose: m.purpose.to_string(),
            url: m.url.to_string(),
            mirror_url: m.mirror_url.map(str::to_string),
            size_bytes: m.size_bytes,
            required_for_semantic: m.required_for_semantic,
        })
        .collect()
}

/// 下载结果。
#[derive(Debug, Serialize)]
pub struct DownloadOutcome {
    /// 模型文件名。
    pub name: String,
    /// 最终路径。
    pub path: String,
    /// 下载字节数。
    pub bytes: u64,
    /// 耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// 一键下载模型。
///
/// 下载过程通过 Tauri 事件 [`EVENT_PROGRESS`] 上报进度；
/// 命令本身在下载完成后才返回，前端可据此判断完成或失败。
#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<DownloadOutcome> {
    // 已在进行中的下载不重复启动
    if state.downloads.is_downloading(&name) {
        return Err(AppError::Invalid(format!("{name} 正在下载中")));
    }

    let started = std::time::Instant::now();
    let app_for_cb = app.clone();
    let name_for_cb = name.clone();

    // 借用期间不能跨 await 持有 MutexGuard，因此先取出目录再重建管理器
    let dir = state.with_models(|m| {
        m.ensure_dir()?;
        Ok(m.models_dir().to_path_buf())
    })?;
    let manager = crate::ml::model_manager::ModelManager::new(dir);

    let path = crate::ml::downloader::download_model(
        &manager,
        &state.downloads,
        &name,
        move |p: DownloadProgress| {
            // 进度事件发不出去不影响下载本身
            if let Err(e) = app_for_cb.emit(EVENT_PROGRESS, &p) {
                log::debug!("上报下载进度失败 ({}): {e}", name_for_cb);
            }
        },
    )
    .await?;

    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    Ok(DownloadOutcome {
        name,
        path: path.to_string_lossy().to_string(),
        bytes,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 取消正在进行的下载。
#[tauri::command]
pub fn cancel_download(state: State<'_, AppState>, name: String) -> Result<bool> {
    Ok(state.downloads.cancel(&name))
}

/// 卸载模型，释放内存。
#[tauri::command]
pub fn unload_model(state: State<'_, AppState>, name: Option<String>) -> Result<()> {
    state.with_models(|m| {
        match name {
            Some(n) => m.unload(&n),
            None => m.unload_all(),
        }
        Ok(())
    })
}

/// 删除已下载的模型文件。
#[tauri::command]
pub fn delete_model(state: State<'_, AppState>, name: String) -> Result<()> {
    state.with_models(|m| m.remove(&name))
}

/// 用系统对话框选择模型目录。
///
/// 返回新目录（已校验可写并写入设置）；用户取消时返回 `None`。
#[tauri::command]
pub fn pick_models_dir(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app.dialog().file().blocking_pick_folder();
    let Some(folder) = picked else {
        return Ok(None);
    };
    let path = folder
        .into_path()
        .map_err(|e| AppError::Invalid(format!("无法解析所选目录: {e}")))?;

    if !crate::ml::model_manager::ModelManager::dir_is_usable(&path) {
        return Err(AppError::Invalid(format!(
            "该目录不可写，请换一个位置: {}",
            path.display()
        )));
    }

    let dir_str = path.to_string_lossy().to_string();
    state.with_conn(|c| crate::db::set_setting(c, KEY_MODELS_DIR, &dir_str))?;
    state.with_models(|m| {
        m.set_dir(path.clone());
        m.ensure_dir()
    })?;

    Ok(Some(dir_str))
}

/// 把模型目录恢复为平台默认位置。
#[tauri::command]
pub fn reset_models_dir(state: State<'_, AppState>) -> Result<String> {
    state.with_conn(|c| crate::db::set_setting(c, KEY_MODELS_DIR, ""))?;
    let default_dir = crate::ml::model_manager::default_models_dir();
    state.with_models(|m| {
        m.set_dir(default_dir.clone());
        m.ensure_dir()
    })?;
    Ok(default_dir.to_string_lossy().to_string())
}

/// 在系统文件管理器中打开模型目录。
#[tauri::command]
pub fn reveal_models_dir(state: State<'_, AppState>) -> Result<String> {
    state.with_models(|m| {
        m.ensure_dir()?;
        Ok(m.models_dir().to_string_lossy().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_list_matches_registry() {
        let list = model_download_list();
        assert_eq!(list.len(), REGISTRY.len());
        for item in &list {
            assert!(item.url.starts_with("https://"));
            assert!(item.mirror_url.is_some(), "{} 应提供备用镜像", item.name);
            assert!(item.size_bytes > 0);
        }
    }

    #[test]
    fn settings_roundtrip_via_state() {
        let conn = crate::db::open_in_memory().unwrap();
        let s = Settings {
            mode: crate::models::AppMode::Full,
            volume: 0.5,
            theme: "mint".into(),
            models_dir: "D:/models".into(),
            ..Default::default()
        };
        s.save(&conn).unwrap();
        let back = Settings::load(&conn).unwrap();
        assert_eq!(back.mode, crate::models::AppMode::Full);
        assert!((back.volume - 0.5).abs() < 1e-6);
        assert_eq!(back.theme, "mint");
        assert_eq!(back.models_dir, "D:/models");
        assert_eq!(
            back.configured_models_dir(),
            std::path::PathBuf::from("D:/models")
        );
    }

    #[test]
    fn empty_models_dir_means_default() {
        let conn = crate::db::open_in_memory().unwrap();
        let s = Settings::default();
        s.save(&conn).unwrap();
        let back = Settings::load(&conn).unwrap();
        assert_eq!(
            back.configured_models_dir(),
            crate::ml::model_manager::default_models_dir(),
            "空串应解析为平台默认目录"
        );
    }

    #[test]
    fn invalid_theme_falls_back_to_default() {
        let conn = crate::db::open_in_memory().unwrap();
        // 直接写库模拟历史脏数据 / 手工修改
        crate::db::set_setting(&conn, crate::utils::config::KEY_THEME, "neon").unwrap();
        assert_eq!(
            Settings::load(&conn).unwrap().theme,
            crate::utils::config::DEFAULT_THEME
        );
        // 保存非法主题也会被规范化
        let s = Settings {
            theme: "bogus".into(),
            ..Default::default()
        };
        s.save(&conn).unwrap();
        assert_eq!(
            Settings::load(&conn).unwrap().theme,
            crate::utils::config::DEFAULT_THEME
        );
    }

    #[test]
    fn themes_list_is_consistent_with_default() {
        let list = list_themes();
        assert_eq!(list.len(), 6, "应有 6 套主题");
        assert!(list.contains(&crate::utils::config::DEFAULT_THEME));
        // 深色/浅色都要有
        assert!(list.contains(&"graphite"));
        assert!(list.contains(&"dawn"));
        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len(), "主题标识不应重复");
    }
}
