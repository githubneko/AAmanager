//! AAmanager —— 本地优先的音频资产管理器，库入口。
//!
//! 本地优先、离线可用的音频资产管理工具。当前实现覆盖策划书 **Phase 1**：
//! 音频导入、播放、波形显示、列表管理、关键词搜索（FTS5）+ 规则引擎自动标签；
//! 并预留了 Phase 3-5（YAMNet / CLAP / 相似度继承）的接入点。
//!
//! # 架构
//!
//! ```text
//! 前端 (Svelte + Tauri WebView)
//!   │  invoke
//!   ▼
//! commands/       Tauri 命令层（薄，只做参数校验与编排）
//!   │
//!   ├─ db/         SQLite（rusqlite，bundled + FTS5）
//!   ├─ audio/      Symphonia 解码 / rubato 重采样 / rustfft 特征 / rodio 播放
//!   ├─ indexer/    后台分析线程与持久化任务队列
//!   └─ ml/         模型管理（惰性加载骨架）+ 余弦相似度
//! ```

pub mod audio;
pub mod commands;
pub mod db;
pub mod error;
pub mod indexer;
pub mod ml;
pub mod models;
pub mod tag_vocab;
pub mod text;
pub mod utils;

use commands::AppState;
use tauri::Manager;

/// 简单的 stderr 日志实现，避免为桌面工具再引入一个日志框架。
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 打开数据库并启动后台索引线程
            let db_path = utils::paths::db_path();
            log::info!("数据库位置: {}", db_path.display());

            match AppState::new(db_path) {
                Ok(state) => {
                    app.manage(state);
                }
                Err(e) => {
                    // 数据库打不开属于致命错误：明确报错而不是静默降级
                    log::error!("初始化失败: {e}");
                    return Err(Box::new(e) as Box<dyn std::error::Error>);
                }
            }

            // 启动时把上次未完成的分析任务叫醒
            if let Some(state) = app.try_state::<AppState>() {
                state.worker.notify();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // ---- 音频导入 / 列表 / 播放 ----
            commands::audio::import_paths,
            commands::audio::import_folder,
            commands::audio::import_files,
            commands::audio::get_waveform,
            commands::audio::get_audio,
            commands::audio::list_audio,
            commands::audio::delete_audio,
            commands::audio::reanalyze,
            commands::audio::analyze_all,
            commands::audio::index_status,
            commands::audio::reveal_in_file_manager,
            commands::audio::play_audio,
            commands::audio::pause_audio,
            commands::audio::resume_audio,
            commands::audio::stop_audio,
            commands::audio::seek_audio,
            commands::audio::set_volume,
            commands::audio::playback_state,
            // ---- 标签 / 备注 ----
            commands::notes::list_tags,
            commands::notes::list_all_tags,
            commands::notes::suggest_tags,
            commands::notes::add_tag,
            commands::notes::add_tag_many,
            commands::notes::remove_tag,
            commands::notes::remove_tag_many,
            commands::notes::confirm_tag,
            commands::notes::rename_tag,
            commands::notes::list_notes,
            commands::notes::set_note,
            commands::notes::set_note_many,
            commands::notes::append_note,
            commands::notes::delete_note,
            commands::notes::bulk_edit,
            commands::notes::library_stats,
            // ---- 标签词汇表 ----
            commands::tags::list_tag_vocabularies,
            commands::tags::set_tag_vocabulary,
            commands::tags::set_tag_override,
            commands::tags::inherit_tags,
            // ---- 搜索 ----
            commands::search::search_audio,
            commands::search::semantic_search,
            commands::search::similar_audio,
            // ---- 设置 / 模型 ----
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::list_themes,
            commands::settings::models_info,
            commands::settings::model_download_list,
            commands::settings::download_model,
            commands::settings::cancel_download,
            commands::settings::unload_model,
            commands::settings::delete_model,
            commands::settings::pick_models_dir,
            commands::settings::reset_models_dir,
            commands::settings::reveal_models_dir,
            // ---- 仅 debug 构建：开发期诊断 ----
            #[cfg(debug_assertions)]
            commands::debug::debug_report_ui,
            #[cfg(debug_assertions)]
            commands::debug::debug_auto_import_dir,
            #[cfg(debug_assertions)]
            commands::debug::debug_write_page_state,
            #[cfg(debug_assertions)]
            commands::debug::debug_should_focus_search,
            #[cfg(debug_assertions)]
            commands::debug::debug_take_commands,
            #[cfg(debug_assertions)]
            commands::debug::debug_write_command_result,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
