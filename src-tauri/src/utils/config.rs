//! 应用设置：读写 `settings` 表，并转换 [`crate::models::AppMode`]。

use crate::db;
use crate::error::Result;
use crate::models::AppMode;
use crate::tag_vocab::TagVocab;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// 设置项键名：运行模式。
pub const KEY_MODE: &str = "mode";
/// 设置项键名：音量。
pub const KEY_VOLUME: &str = "volume";
/// 设置项键名：上次导入目录。
pub const KEY_LAST_IMPORT_DIR: &str = "last_import_dir";
/// 设置项键名：导入时是否自动开始分析。
pub const KEY_AUTO_ANALYZE: &str = "auto_analyze";
/// 设置项键名：波形峰值点数。
pub const KEY_WAVEFORM_POINTS: &str = "waveform_points";
/// 设置项键名：自定义模型目录（空串表示用平台默认目录）。
pub const KEY_MODELS_DIR: &str = "models_dir";
/// 设置项键名：界面主题。
pub const KEY_THEME: &str = "theme";
/// 设置项键名：标签词汇表（game / video / music / custom）。
pub const KEY_TAG_VOCAB: &str = "tag_vocab";
/// 设置项键名：自定义词汇表的重命名表（JSON 对象）。
pub const KEY_TAG_OVERRIDES: &str = "tag_overrides";

/// 内置主题标识（必须与前端 `src/lib/theme.ts` 的列表一致）。
pub const THEMES: &[&str] = &[
    "graphite", // 深色 · 石墨
    "midnight", // 深色 · 靛蓝
    "forest",   // 深色 · 松绿
    "dawn",     // 浅色 · 晨雾
    "sand",     // 浅色 · 暖沙
    "mint",     // 浅色 · 薄荷
];

/// 默认主题（与前端默认值一致）。
pub const DEFAULT_THEME: &str = "graphite";

/// 前端可见的完整设置快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 轻量模式 / 完整模式。
    pub mode: AppMode,
    /// 播放音量 0.0 - 1.0。
    pub volume: f32,
    /// 上次使用的导入目录。
    pub last_import_dir: String,
    /// 导入后是否自动排队分析。
    pub auto_analyze: bool,
    /// 波形峰值点数。
    pub waveform_points: u32,
    /// 界面主题标识。
    pub theme: String,
    /// 自定义模型目录；空串表示使用平台默认目录。
    pub models_dir: String,
    /// 标签词汇表。
    pub tag_vocab: TagVocab,
    /// 自定义词汇表的重命名表（规范标签 -> 用户说法）。
    pub tag_overrides: std::collections::HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: AppMode::Lite,
            volume: 0.8,
            last_import_dir: String::new(),
            auto_analyze: true,
            waveform_points: crate::audio::waveform::DEFAULT_PEAK_COUNT as u32,
            theme: DEFAULT_THEME.to_string(),
            models_dir: String::new(),
            tag_vocab: TagVocab::default(),
            tag_overrides: std::collections::HashMap::new(),
        }
    }
}

impl Settings {
    /// 从数据库读取；缺项用默认值补齐。
    pub fn load(conn: &Connection) -> Result<Self> {
        let d = Settings::default();
        let mode = db::get_setting(conn, KEY_MODE)?
            .and_then(|v| match v.as_str() {
                "full" => Some(AppMode::Full),
                "lite" => Some(AppMode::Lite),
                _ => None,
            })
            .unwrap_or(d.mode);

        Ok(Self {
            mode,
            volume: db::get_setting(conn, KEY_VOLUME)?
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| (0.0..=1.0).contains(v))
                .unwrap_or(d.volume),
            last_import_dir: db::get_setting(conn, KEY_LAST_IMPORT_DIR)?.unwrap_or_default(),
            auto_analyze: db::get_setting(conn, KEY_AUTO_ANALYZE)?
                .map(|v| v != "false" && v != "0")
                .unwrap_or(d.auto_analyze),
            waveform_points: db::get_setting(conn, KEY_WAVEFORM_POINTS)?
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|v| (64..=8192).contains(v))
                .unwrap_or(d.waveform_points),
            // 未知主题回退到默认值，避免前端拿到无效标识
            theme: db::get_setting(conn, KEY_THEME)?
                .filter(|v| THEMES.contains(&v.as_str()))
                .unwrap_or(d.theme),
            models_dir: db::get_setting(conn, KEY_MODELS_DIR)?.unwrap_or_default(),
            tag_vocab: db::get_setting(conn, KEY_TAG_VOCAB)?
                .map(|v| TagVocab::from_db(&v))
                .unwrap_or_default(),
            tag_overrides: db::get_setting(conn, KEY_TAG_OVERRIDES)?
                .and_then(|v| serde_json::from_str(&v).ok())
                .unwrap_or_default(),
        })
    }

    /// 写回数据库。
    pub fn save(&self, conn: &Connection) -> Result<()> {
        db::set_setting(
            conn,
            KEY_MODE,
            match self.mode {
                AppMode::Lite => "lite",
                AppMode::Full => "full",
            },
        )?;
        db::set_setting(conn, KEY_VOLUME, &self.volume.clamp(0.0, 1.0).to_string())?;
        db::set_setting(conn, KEY_LAST_IMPORT_DIR, &self.last_import_dir)?;
        db::set_setting(
            conn,
            KEY_AUTO_ANALYZE,
            if self.auto_analyze { "true" } else { "false" },
        )?;
        db::set_setting(conn, KEY_WAVEFORM_POINTS, &self.waveform_points.to_string())?;
        // 只接受内置主题，其余落回默认值
        let theme = if THEMES.contains(&self.theme.as_str()) {
            self.theme.clone()
        } else {
            DEFAULT_THEME.to_string()
        };
        db::set_setting(conn, KEY_THEME, &theme)?;
        db::set_setting(conn, KEY_MODELS_DIR, &self.models_dir)?;
        db::set_setting(conn, KEY_TAG_VOCAB, self.tag_vocab.as_db())?;
        db::set_setting(
            conn,
            KEY_TAG_OVERRIDES,
            &serde_json::to_string(&self.tag_overrides).unwrap_or_else(|_| "{}".into()),
        )?;
        Ok(())
    }

    /// 构建当前词汇表的解析器。
    pub fn tag_resolver(&self) -> crate::tag_vocab::Resolver {
        crate::tag_vocab::Resolver::new(self.tag_vocab, self.tag_overrides.clone())
    }

    /// 有效的模型目录：环境变量 > 自定义目录 > 平台默认目录。
    ///
    /// 环境变量 `AAM_MODELS_DIR` 优先级最高 —— 便于自动化验证与「绿色版」
    /// 免安装部署把模型放到指定位置。
    pub fn effective_models_dir(&self) -> std::path::PathBuf {
        if let Ok(env_dir) = std::env::var("AAM_MODELS_DIR") {
            let env_dir = env_dir.trim();
            if !env_dir.is_empty() {
                return std::path::PathBuf::from(env_dir);
            }
        }
        let custom = self.models_dir.trim();
        if custom.is_empty() {
            crate::ml::model_manager::default_models_dir()
        } else {
            std::path::PathBuf::from(custom)
        }
    }

    /// 在**不考虑**环境变量覆盖的前提下，设置项本身指向的目录。
    ///
    /// 用于测试与调试：`effective_models_dir()` 会被 `AAM_MODELS_DIR` 覆盖，
    /// 因此断言「设置项是否被正确保存」时需要这个不含环境变量的版本。
    pub fn configured_models_dir(&self) -> std::path::PathBuf {
        let custom = self.models_dir.trim();
        if custom.is_empty() {
            crate::ml::model_manager::default_models_dir()
        } else {
            std::path::PathBuf::from(custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn defaults_are_sane() {
        let conn = open_in_memory().unwrap();
        let s = Settings::load(&conn).unwrap();
        assert_eq!(s.mode, AppMode::Lite);
        assert!(s.auto_analyze, "默认应自动分析（不阻塞 UI）");
        assert!((s.volume - 0.8).abs() < 1e-6);
        assert_eq!(s.waveform_points, 1024);
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let conn = open_in_memory().unwrap();
        let s = Settings {
            mode: AppMode::Full,
            volume: 0.35,
            last_import_dir: "C:/sfx".into(),
            auto_analyze: false,
            waveform_points: 2048,
            theme: "sand".into(),
            models_dir: "E:/models".into(),
            tag_vocab: TagVocab::Music,
            tag_overrides: [("沉闷".to_string(), "我的低音".to_string())]
                .into_iter()
                .collect(),
        };
        s.save(&conn).unwrap();

        let loaded = Settings::load(&conn).unwrap();
        assert_eq!(loaded.mode, AppMode::Full);
        assert!((loaded.volume - 0.35).abs() < 1e-6);
        assert_eq!(loaded.last_import_dir, "C:/sfx");
        assert!(!loaded.auto_analyze);
        assert_eq!(loaded.waveform_points, 2048);
    }

    #[test]
    fn invalid_stored_values_fall_back_to_defaults() {
        let conn = open_in_memory().unwrap();
        db::set_setting(&conn, KEY_MODE, "nonsense").unwrap();
        db::set_setting(&conn, KEY_VOLUME, "5.0").unwrap(); // 超出范围
        db::set_setting(&conn, KEY_VOLUME, "abc").unwrap(); // 非数字
        db::set_setting(&conn, KEY_WAVEFORM_POINTS, "999999").unwrap(); // 超出范围

        let s = Settings::load(&conn).unwrap();
        assert_eq!(s.mode, AppMode::Lite);
        assert!((s.volume - 0.8).abs() < 1e-6);
        assert_eq!(s.waveform_points, 1024);
    }

    #[test]
    fn volume_is_clamped_on_save() {
        let conn = open_in_memory().unwrap();
        let s = Settings {
            volume: 9.0,
            ..Default::default()
        };
        s.save(&conn).unwrap();
        let loaded = Settings::load(&conn).unwrap();
        assert!(loaded.volume <= 1.0);
    }

    #[test]
    fn auto_analyze_parses_common_false_forms() {
        let conn = open_in_memory().unwrap();
        for v in ["false", "0"] {
            db::set_setting(&conn, KEY_AUTO_ANALYZE, v).unwrap();
            assert!(
                !Settings::load(&conn).unwrap().auto_analyze,
                "值 {v} 应视为关闭"
            );
        }
        for v in ["true", "1", "yes"] {
            db::set_setting(&conn, KEY_AUTO_ANALYZE, v).unwrap();
            assert!(
                Settings::load(&conn).unwrap().auto_analyze,
                "值 {v} 应视为开启"
            );
        }
    }

    /// `AAM_MODELS_DIR` 必须能覆盖设置项（绿色版部署与自动化验证依赖这一点）。
    ///
    /// 注意：同一进程内并发跑测试时改环境变量会有竞争，因此这里只断言
    /// 三种取值下的映射关系，不做跨线程共享。
    #[test]
    fn env_var_overrides_configured_models_dir() {
        let s = Settings {
            models_dir: "D:/models".into(),
            ..Default::default()
        };

        std::env::remove_var("AAM_MODELS_DIR");
        assert_eq!(
            s.effective_models_dir(),
            std::path::PathBuf::from("D:/models"),
            "无环境变量时应使用设置项"
        );

        std::env::set_var("AAM_MODELS_DIR", "E:/portable-models");
        assert_eq!(
            s.effective_models_dir(),
            std::path::PathBuf::from("E:/portable-models"),
            "环境变量应优先于设置项"
        );

        // 纯空白视为未设置
        std::env::set_var("AAM_MODELS_DIR", "   ");
        assert_eq!(
            s.effective_models_dir(),
            std::path::PathBuf::from("D:/models"),
            "空白环境变量不应生效"
        );

        std::env::remove_var("AAM_MODELS_DIR");
    }

    /// `configured_models_dir` 只看设置项，不受环境变量影响。
    #[test]
    fn configured_models_dir_ignores_env_var() {
        std::env::set_var("AAM_MODELS_DIR", "E:/portable-models");
        let s = Settings {
            models_dir: "D:/models".into(),
            ..Default::default()
        };
        assert_eq!(
            s.configured_models_dir(),
            std::path::PathBuf::from("D:/models")
        );
        std::env::remove_var("AAM_MODELS_DIR");
    }
}
