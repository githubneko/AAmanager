//! 应用共享状态：数据库连接、后台索引线程、播放器、模型管理器。

use crate::audio::player::Player;
use crate::error::Result;
use crate::indexer::Worker;
use crate::ml::downloader::DownloadRegistry;
use crate::ml::model_manager::ModelManager;
use crate::utils::config::Settings;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Mutex;

/// Tauri 命令层共享的状态。
///
/// * `conn` —— 主数据库连接。SQLite 本身串行化写入，这里加 `Mutex` 即可；
/// * `worker` —— 后台分析线程，拥有自己的数据库连接；
/// * `player` —— 惰性创建的播放器（符合「启动 < 1 秒」目标，
///   没有播放需求时不会去碰音频设备）；
/// * `models` —— 模型管理器，目录取自用户设置；下载任务另行登记在
///   [`crate::commands::state::AppState::downloads`] 中以便取消。
pub struct AppState {
    /// 主数据库连接。
    pub conn: Mutex<Connection>,
    /// 后台索引线程。
    pub worker: Worker,
    /// 惰性创建的播放器。
    pub player: Mutex<Option<Player>>,
    /// 模型管理器（目录可被用户更改，故需要可变访问）。
    pub models: Mutex<ModelManager>,
    /// 正在进行的模型下载（用于取消与状态查询）。
    pub downloads: DownloadRegistry,
}

impl AppState {
    /// 打开数据库、启动索引线程并构造状态。
    pub fn new(db_path: PathBuf) -> Result<Self> {
        let conn = crate::db::open(&db_path)?;
        let worker = Worker::start(db_path);

        // 模型目录取自用户设置，未设置时用平台默认目录
        let models_dir = Settings::load(&conn)?.effective_models_dir();
        let models = ModelManager::new(models_dir);
        // 目录先建好，方便用户按提示手动放置模型文件
        if let Err(e) = models.ensure_dir() {
            log::warn!("模型目录不可用（可稍后在设置中更改）: {e}");
        }

        Ok(Self {
            conn: Mutex::new(conn),
            worker,
            player: Mutex::new(None),
            models: Mutex::new(models),
            downloads: DownloadRegistry::new(),
        })
    }

    /// 借用主数据库连接。
    ///
    /// 锁中毒（某次命令 panic）时不让整个应用失效，而是恢复内部值继续可用。
    pub fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    /// 借用可写的主数据库连接（事务需要 `&mut Connection`）。
    pub fn with_conn_mut<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// 借用模型管理器。
    pub fn with_models<T>(&self, f: impl FnOnce(&mut ModelManager) -> Result<T>) -> Result<T> {
        let mut guard = self.models.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// 确保播放器已创建并对其执行操作。
    ///
    /// 首次调用会打开音频输出设备，因此放在这里而不是启动流程里。
    pub fn with_player<T>(&self, f: impl FnOnce(&mut Player) -> Result<T>) -> Result<T> {
        let mut guard = self.player.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Player::new()?);
        }
        let p = guard
            .as_mut()
            .ok_or_else(|| crate::error::AppError::Audio("播放器初始化失败".into()))?;
        f(p)
    }
}
