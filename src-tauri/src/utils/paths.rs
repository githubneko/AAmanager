//! 跨平台路径解析。
//!
//! 不引入 `dirs` 依赖，直接用环境变量判断，减少打包体积（策划书 8：
//! 安装包 < 20 MB）。
//!
//! * Windows —— `%APPDATA%\audio-manager`
//! * macOS   —— `~/Library/Application Support/audio-manager`
//! * Linux   —— `$XDG_DATA_HOME/audio-manager` 或 `~/.local/share/audio-manager`

use std::path::PathBuf;

/// 应用数据目录名。
pub const APP_DIR_NAME: &str = "audio-manager";

/// 用户主目录。
pub fn home_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// 应用数据根目录。
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }
    } else if cfg!(target_os = "macos") {
        return home_dir()
            .join("Library")
            .join("Application Support")
            .join(APP_DIR_NAME);
    } else {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            let p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                return p.join(APP_DIR_NAME);
            }
        }
        return home_dir().join(".local").join("share").join(APP_DIR_NAME);
    }

    // 非 Windows 但 APPDATA 缺失，或 Windows 且 APPDATA 缺失时的兜底
    home_dir().join(format!(".{APP_DIR_NAME}"))
}

/// SQLite 数据库文件路径。
pub fn db_path() -> PathBuf {
    data_dir().join("library.db")
}

/// 模型目录。
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// 日志目录。
pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// 确保数据目录存在。
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let d = data_dir();
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// 取路径最后一段作为文件名（跨平台，失败时回退为整个路径）。
pub fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// 取父目录字符串。
pub fn parent_of(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_ends_with_app_name() {
        let d = data_dir();
        assert_eq!(d.file_name().unwrap().to_string_lossy(), APP_DIR_NAME);
        assert!(
            d.is_absolute() || d.starts_with("."),
            "应返回绝对路径或兜底相对路径"
        );
    }

    #[test]
    fn db_and_models_live_under_data_dir() {
        let d = data_dir();
        assert_eq!(db_path().parent().unwrap(), d);
        assert_eq!(models_dir().parent().unwrap(), d);
        assert_eq!(log_dir().parent().unwrap(), d);
        assert_eq!(
            db_path().file_name().unwrap().to_string_lossy(),
            "library.db"
        );
    }

    #[test]
    fn file_name_and_parent_helpers() {
        assert_eq!(file_name_of("C:/sfx/gunshot.wav"), "gunshot.wav");
        assert_eq!(parent_of("C:/sfx/gunshot.wav").replace('\\', "/"), "C:/sfx");
        // 无扩展名 / 无目录时不应 panic
        assert_eq!(file_name_of("plain"), "plain");
        let _ = parent_of("plain");
    }

    #[test]
    #[cfg(windows)]
    fn windows_uses_appdata_when_present() {
        if std::env::var_os("APPDATA").is_some() {
            let d = data_dir();
            assert!(
                d.to_string_lossy().to_lowercase().contains("appdata"),
                "Windows 下应基于 APPDATA: {}",
                d.display()
            );
        }
    }
}
