//! 统一错误类型。
//!
//! 所有 Tauri 命令都返回 [`Result`]，错误会被序列化成字符串传给前端。

use serde::{Serialize, Serializer};

/// 应用统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 数据库相关错误。
    #[error("数据库错误: {0}")]
    Database(#[from] rusqlite::Error),

    /// 音频解码 / 播放相关错误。
    #[error("音频错误: {0}")]
    Audio(String),

    /// 文件系统相关错误。
    #[error("文件错误: {0}")]
    Io(#[from] std::io::Error),

    /// JSON 序列化错误。
    #[error("序列化错误: {0}")]
    Json(#[from] serde_json::Error),

    /// 参数非法。
    #[error("参数错误: {0}")]
    Invalid(String),

    /// 找不到请求的资源。
    #[error("未找到: {0}")]
    NotFound(String),

    /// 尚未实现（例如模型推理模块）。
    #[error("尚未实现: {0}")]
    NotImplemented(String),
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// 应用统一 Result 别名。
pub type Result<T> = std::result::Result<T, AppError>;
