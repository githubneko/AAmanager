//! 模型注册表与惰性加载管理。
//!
//! 设计目标（策划书 2.3 / 4.8）：
//! * 启动时**不加载任何模型**，仅检查文件是否存在；
//! * 首次使用语义搜索时才下载 / 载入，并上报进度；
//! * 卸载后释放内存。
//!
//! 模型文件本身按用户决定暂不下载，本模块先把「发现 → 下载 → 加载 → 卸载」
//! 的骨架和磁盘布局固定下来，后续接入 ONNX Runtime 时只替换 `load` 内部实现。

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 一个模型条目的静态描述。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    /// 逻辑名称（同时是磁盘上的文件名）。
    pub name: &'static str,
    /// 展示名。
    pub display_name: &'static str,
    /// 用途说明。
    pub purpose: &'static str,
    /// 下载地址（Hugging Face 或自建镜像）。
    pub url: &'static str,
    /// 文件大小（字节，用于显示进度与预估下载时间）。
    pub size_bytes: u64,
    /// 是否为语义搜索（完整模式）所必需。
    pub required_for_semantic: bool,
    /// 备注 / 备用源。
    pub mirror_url: Option<&'static str>,
}

/// 模型注册表。
///
/// **URL 均已实测可达**（HTTP 200，见 README）。与策划书 3.3 节的差异及原因：
///
/// * 策划书给出的 `laion/clap-htsat-unfused/onnx/*.onnx` 与
///   `onnx-community/yamnet` 路径实测为 404 / 401，不可用；这里改用社区
///   已导出 ONNX 的仓库（`Xenova/*`）。
///
/// ## 为什么 CLAP 用 fp32 而不是 INT8（实测结论）
///
/// 策划书 3.3 建议量化以缩小体积，我最初也照做了（音频 33 MB / 文本 121 MB）。
/// 但实测发现**动态量化破坏了向量质量**：
///
/// * 同一个输入下，量化文本塔与 PyTorch fp32 的嵌入余弦只有 **0.829**，
///   而 fp32 导出与 PyTorch **逐位一致（1.000000）**；
/// * 用真实素材做检索，量化文本塔 Top-1 命中 **1/6**，换 fp32 后
///   **2/3 且置信度很高**（如「a ringing bell」对 ding 得 +5.67）。
///
/// **音频塔则用 INT8**：实测 INT8 与 fp32 的检索质量几乎无差
/// （平均极差 0.428 vs 0.432，每个素材的最匹配提示词完全一致），
/// 而体积只有 1/3.6。带宽花在文本塔上更值 —— 那里量化会真的毁掉检索。
///
/// 最终下载量：音频 32.7 MB + 文本 501.5 MB + 分词器 2.1 MB ≈ 536 MB
/// （若音频塔也用 fp32 则是 616 MB，多花 80 MB 换 1% 的质量，不划算）。
///
/// ## AST 分类器
///
/// 音频事件分类用 **AST**（Audio Spectrogram Transformer）而非 YAMNet：
/// YAMNet 的 ONNX 导出在当前可访问的仓库中不存在，而 AST 同样是 AudioSet
/// 多标签分类器（527 类）且精度更高。为与策划书保持一致的调用面，
/// 磁盘文件名仍为 `yamnet.onnx`。
pub const REGISTRY: &[ModelSpec] = &[
    ModelSpec {
        name: "yamnet.onnx",
        display_name: "音频事件分类（AST / AudioSet）",
        purpose: "自动标签：识别枪声、爆炸、点击等 527 类音频事件",
        url: "https://huggingface.co/Xenova/ast-finetuned-audioset-10-10-0.4593/resolve/main/onnx/model_quantized.onnx",
        size_bytes: 90_800_000,
        required_for_semantic: false,
        mirror_url: Some(
            "https://hf-mirror.com/Xenova/ast-finetuned-audioset-10-10-0.4593/resolve/main/onnx/model_quantized.onnx",
        ),
    },
    ModelSpec {
        // 音频塔用 INT8：实测与 fp32 的检索质量几乎无差
        // （平均极差 0.428 vs 0.432，最匹配项完全一致），
        // 但体积只有 1/3.6（32.7 MB vs 112 MB）。带宽花在文本塔上更值。
        name: "clap_audio.onnx",
        display_name: "CLAP 音频编码器",
        purpose: "音频向量化（512 维），用于语义搜索与相似音频",
        url: "https://huggingface.co/Xenova/clap-htsat-unfused/resolve/main/onnx/audio_model_quantized.onnx",
        size_bytes: 34_301_667,
        required_for_semantic: true,
        mirror_url: Some(
            "https://hf-mirror.com/Xenova/clap-htsat-unfused/resolve/main/onnx/audio_model_quantized.onnx",
        ),
    },
    ModelSpec {
        name: "clap_text.onnx",
        display_name: "CLAP 文本编码器（fp32）",
        purpose: "自然语言查询向量化（512 维）",
        url: "https://huggingface.co/Xenova/clap-htsat-unfused/resolve/main/onnx/text_model.onnx",
        size_bytes: 501_513_769,
        required_for_semantic: true,
        mirror_url: Some(
            "https://hf-mirror.com/Xenova/clap-htsat-unfused/resolve/main/onnx/text_model.onnx",
        ),
    },
    ModelSpec {
        name: "tokenizer.json",
        display_name: "CLAP 分词器（RoBERTa BPE）",
        purpose: "把中文/英文查询切成模型输入 token（纯 Rust 推理，无需 Python）",
        url: "https://huggingface.co/Xenova/clap-htsat-unfused/resolve/main/tokenizer.json",
        size_bytes: 2_108_774,
        required_for_semantic: true,
        mirror_url: Some(
            "https://hf-mirror.com/Xenova/clap-htsat-unfused/resolve/main/tokenizer.json",
        ),
    },
];

/// 某个模型在磁盘上的状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    /// 逻辑名称。
    pub name: String,
    /// 展示名。
    pub display_name: String,
    /// 用途。
    pub purpose: String,
    /// 是否已安装。
    pub installed: bool,
    /// 是否已加载进内存。
    pub loaded: bool,
    /// 磁盘路径。
    pub path: String,
    /// 期望大小（字节）。
    pub expected_size: u64,
    /// 实际大小（字节，未安装为 0）。
    pub actual_size: u64,
    /// 实际大小是否与注册表登记的一致。
    ///
    /// 用来发现「装的是别的版本」——例如注册表要 fp32 的文本塔（501.5 MB），
    /// 而磁盘上是 INT8 量化版（120.7 MB）。两者**文件名相同**，
    /// 只查存在性会显示「已安装」，但量化版会让检索命中率从 2/3 掉到 1/6，
    /// 用户却完全看不出来。
    pub up_to_date: bool,
    /// 下载地址。
    pub url: String,
}

/// 实际文件大小是否可视为与登记值一致（容忍 1% 误差）。
///
/// 未安装（`actual == 0`）时返回 `false` —— 调用方应结合 `installed` 判断。
fn is_size_matching(actual: u64, expected: u64) -> bool {
    if actual == 0 || expected == 0 {
        return false;
    }
    let diff = actual.abs_diff(expected);
    diff * 100 <= expected // <= 1%
}

/// 模型管理器。
///
/// `loaded` 只记录哪些模型被载入过；真正的 ONNX `Session` 在接入
/// `ort` 之后放进同一个 map（见 `Session` 类型别名处的说明）。
pub struct ModelManager {
    /// 模型存放目录。
    models_dir: PathBuf,
    /// 已加载模型的名称集合。
    loaded: Mutex<Vec<String>>,
}

impl ModelManager {
    /// 用指定目录创建管理器。
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            models_dir,
            loaded: Mutex::new(Vec::new()),
        }
    }

    /// 用平台默认目录创建管理器。
    pub fn with_default_dir() -> Self {
        Self::new(default_models_dir())
    }

    /// 按用户配置创建管理器：环境变量 > 设置项 > 平台默认目录。
    ///
    /// 供后台分析线程等无法访问 `AppState` 的场景使用。
    pub fn with_configured_dir(conn: &rusqlite::Connection) -> Self {
        if let Ok(env_dir) = std::env::var("AAM_MODELS_DIR") {
            let env_dir = env_dir.trim();
            if !env_dir.is_empty() {
                return Self::new(PathBuf::from(env_dir));
            }
        }
        let configured = crate::db::get_setting(conn, crate::utils::config::KEY_MODELS_DIR)
            .ok()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        Self::new(configured.unwrap_or_else(default_models_dir))
    }

    /// 模型目录。
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// 切换到新的模型目录（用户自定义目录时调用）。
    ///
    /// 已加载的模型会被卸载 —— 它们来自旧目录，继续持有会与新目录不一致。
    pub fn set_dir(&mut self, dir: PathBuf) {
        if self.models_dir == dir {
            return;
        }
        self.unload_all();
        self.models_dir = dir;
    }

    /// 目录是否可写（切换目录前校验，避免选到一个只读位置）。
    pub fn dir_is_usable(dir: &Path) -> bool {
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        // 真正试写一个文件，比只看权限位可靠
        let probe = dir.join(".aam_write_test");
        match std::fs::write(&probe, b"ok") {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    /// 确保目录存在。
    pub fn ensure_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.models_dir)?;
        Ok(())
    }

    /// 某个模型在磁盘上的绝对路径。
    pub fn model_path(&self, name: &str) -> PathBuf {
        self.models_dir.join(name)
    }

    /// 按名称查找注册表条目。
    pub fn spec(name: &str) -> Option<&'static ModelSpec> {
        REGISTRY.iter().find(|m| m.name == name)
    }

    /// 模型是否已存在于磁盘。
    pub fn is_installed(&self, name: &str) -> bool {
        self.model_path(name).is_file()
    }

    /// 是否已加载进内存。
    pub fn is_loaded(&self, name: &str) -> bool {
        self.loaded
            .lock()
            .map(|l| l.iter().any(|n| n == name))
            .unwrap_or(false)
    }

    /// 列出全部模型的当前状态。
    pub fn statuses(&self) -> Vec<ModelStatus> {
        REGISTRY
            .iter()
            .map(|spec| {
                let path = self.model_path(spec.name);
                let actual = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                ModelStatus {
                    name: spec.name.to_string(),
                    display_name: spec.display_name.to_string(),
                    purpose: spec.purpose.to_string(),
                    installed: actual > 0,
                    loaded: self.is_loaded(spec.name),
                    path: path.to_string_lossy().to_string(),
                    expected_size: spec.size_bytes,
                    actual_size: actual,
                    up_to_date: is_size_matching(actual, spec.size_bytes),
                    url: spec.url.to_string(),
                }
            })
            .collect()
    }

    /// 语义搜索所需的模型是否齐备。
    pub fn semantic_ready(&self) -> bool {
        REGISTRY
            .iter()
            .filter(|m| m.required_for_semantic)
            .all(|m| self.is_installed(m.name))
    }

    /// 确保模型文件存在；缺失则返回需要下载的信息（不在此处执行下载）。
    ///
    /// 之所以不在这里直接下载，是为了让调用方（Tauri 命令）能够向前端
    /// 上报进度并支持取消。
    pub fn ensure_available(&self, name: &str) -> Result<PathBuf> {
        let spec =
            Self::spec(name).ok_or_else(|| AppError::Invalid(format!("未知模型: {name}")))?;
        let path = self.model_path(name);
        if path.is_file() {
            return Ok(path);
        }
        Err(AppError::NotFound(format!(
            "模型 {} ({}) 尚未安装，需从 {} 下载（约 {:.1} MB），或手动放入目录 {}",
            spec.display_name,
            spec.name,
            spec.url,
            spec.size_bytes as f64 / 1_048_576.0,
            self.models_dir.display()
        )))
    }

    /// 惰性加载模型。模型不存在时返回可读的错误（由调用方决定是否提示下载）。
    pub fn load(&self, name: &str) -> Result<PathBuf> {
        let path = self.ensure_available(name)?;
        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| AppError::Audio("模型状态锁已损坏".into()))?;
        if !loaded.iter().any(|n| n == name) {
            // 接入 ONNX Runtime 后，此处创建并缓存 ort::Session：
            //   let session = Session::builder()?.commit_from_file(&path)?;
            //   self.sessions.lock()?.insert(name.to_string(), session);
            loaded.push(name.to_string());
            log::info!("模型 {name} 已登记加载（推理内核待接入）");
        }
        Ok(path)
    }

    /// 卸载模型，释放内存。
    pub fn unload(&self, name: &str) {
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.retain(|n| n != name);
        }
    }

    /// 卸载全部模型。
    pub fn unload_all(&self) {
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.clear();
        }
    }

    /// 删除已下载的模型文件。
    pub fn remove(&self, name: &str) -> Result<()> {
        self.unload(name);
        let path = self.model_path(name);
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// 平台默认模型目录：
/// * Windows —— `%APPDATA%\audio-manager\models`
/// * macOS   —— `~/Library/Application Support/audio-manager/models`
/// * Linux   —— `$XDG_DATA_HOME/audio-manager/models` 或 `~/.local/share/audio-manager/models`
pub fn default_models_dir() -> PathBuf {
    crate::utils::paths::data_dir().join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_models_{}_{}", std::process::id(), tag));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn registry_is_well_formed() {
        assert_eq!(REGISTRY.len(), 4);
        for m in REGISTRY {
            assert!(!m.name.is_empty());
            assert!(m.url.starts_with("http"), "{} 缺少下载地址", m.name);
            assert!(m.size_bytes > 0);
        }
        assert!(REGISTRY.iter().any(|m| m.name == "yamnet.onnx"));
        // 语义检索需要三个文件：音频塔、文本塔、分词器
        let semantic: Vec<&str> = REGISTRY
            .iter()
            .filter(|m| m.required_for_semantic)
            .map(|m| m.name)
            .collect();
        assert!(semantic.contains(&"clap_audio.onnx"));
        assert!(semantic.contains(&"clap_text.onnx"));
        assert!(
            semantic.contains(&"tokenizer.json"),
            "缺分词器时纯 Rust 文本编码无法工作"
        );
    }

    /// 注册表里的文件名必须与代码里使用的常量一致，否则会「下载了却找不到」。
    #[test]
    fn registry_names_match_code_constants() {
        let names: Vec<&str> = REGISTRY.iter().map(|m| m.name).collect();
        assert!(names.contains(&crate::ml::clap_audio::MODEL_NAME));
        assert!(names.contains(&crate::ml::clap_text::MODEL_NAME));
        assert!(names.contains(&crate::ml::clap_text::TOKENIZER_NAME));
    }

    /// 大小校验：用来发现「装的是别的版本」。
    ///
    /// 实测场景：注册表要 fp32 文本塔（501.5 MB），磁盘上是 INT8 版
    /// （120.7 MB）。文件名相同，只查存在性会误报「已安装」。
    #[test]
    fn size_mismatch_is_detected() {
        // 完全一致 / 1% 以内视为正常
        assert!(is_size_matching(1_000, 1_000));
        assert!(is_size_matching(1_005, 1_000), "0.5% 偏差应容忍");
        assert!(is_size_matching(995, 1_000));
        // 量化版 vs fp32 这种量级差异必须被发现
        assert!(
            !is_size_matching(120_700_000, 501_500_000),
            "INT8 与 fp32 的体积差异必须被识别"
        );
        assert!(!is_size_matching(34_301_667, 117_528_416), "音频塔同理");
        // 未安装
        assert!(!is_size_matching(0, 1_000));
    }

    /// 状态里要带上大小校验结果。
    #[test]
    fn status_reports_size_mismatch() {
        let dir = temp_dir("mismatch");
        let mgr = ModelManager::new(dir.clone());
        // 造一个大小与登记值差很远的假文件
        std::fs::write(mgr.model_path("clap_text.onnx"), vec![0u8; 1024]).unwrap();

        let st = mgr.statuses();
        let t = st.iter().find(|s| s.name == "clap_text.onnx").unwrap();
        assert!(t.installed, "文件存在即算已安装");
        assert!(!t.up_to_date, "但大小不符，应标记为版本不一致");
        assert_eq!(t.actual_size, 1024);
        assert!(t.expected_size > 1_000_000);

        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn statuses_report_not_installed_initially() {
        let dir = temp_dir("status");
        let mgr = ModelManager::new(dir.clone());
        let st = mgr.statuses();
        assert_eq!(st.len(), REGISTRY.len());
        assert!(st.iter().all(|s| !s.installed && !s.loaded));
        assert!(!mgr.semantic_ready());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detecting_installed_model() {
        let dir = temp_dir("installed");
        let mgr = ModelManager::new(dir.clone());
        std::fs::write(mgr.model_path("yamnet.onnx"), b"fake-onnx-bytes").unwrap();

        let st = mgr.statuses();
        let y = st.iter().find(|s| s.name == "yamnet.onnx").unwrap();
        assert!(y.installed);
        assert_eq!(y.actual_size, 15);
        assert!(!y.loaded);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_available_errors_with_actionable_message() {
        let dir = temp_dir("ensure");
        let mgr = ModelManager::new(dir.clone());
        let err = mgr
            .ensure_available("clap_text.onnx")
            .unwrap_err()
            .to_string();
        assert!(err.contains("尚未安装"), "{err}");
        assert!(err.contains("http"), "错误信息应包含下载地址: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ensure_available_unknown_model_is_rejected() {
        let dir = temp_dir("unknown");
        let mgr = ModelManager::new(dir.clone());
        assert!(mgr.ensure_available("nope.onnx").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_and_unload_tracks_state() {
        let dir = temp_dir("load");
        let mgr = ModelManager::new(dir.clone());
        std::fs::write(mgr.model_path("yamnet.onnx"), b"x").unwrap();

        assert!(!mgr.is_loaded("yamnet.onnx"));
        let p = mgr.load("yamnet.onnx").unwrap();
        assert!(p.exists());
        assert!(mgr.is_loaded("yamnet.onnx"));

        mgr.unload("yamnet.onnx");
        assert!(!mgr.is_loaded("yamnet.onnx"));

        mgr.load("yamnet.onnx").unwrap();
        mgr.unload_all();
        assert!(!mgr.is_loaded("yamnet.onnx"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_deletes_file_and_state() {
        let dir = temp_dir("remove");
        let mgr = ModelManager::new(dir.clone());
        std::fs::write(mgr.model_path("yamnet.onnx"), b"x").unwrap();
        mgr.load("yamnet.onnx").unwrap();

        mgr.remove("yamnet.onnx").unwrap();
        assert!(!mgr.is_installed("yamnet.onnx"));
        assert!(!mgr.is_loaded("yamnet.onnx"));
        // 重复删除不应报错
        mgr.remove("yamnet.onnx").unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn semantic_ready_requires_all_three_files() {
        let dir = temp_dir("semantic");
        let mgr = ModelManager::new(dir.clone());

        // 只装音频塔：不够
        std::fs::write(mgr.model_path("clap_audio.onnx"), b"x").unwrap();
        assert!(!mgr.semantic_ready(), "只装音频编码器还不够");

        // 再加文本塔：仍不够（缺分词器，纯 Rust 编码无法工作）
        std::fs::write(mgr.model_path("clap_text.onnx"), b"x").unwrap();
        assert!(
            !mgr.semantic_ready(),
            "缺分词器时文本无法编码，不应报告就绪"
        );

        // 三个齐全才算就绪
        std::fs::write(mgr.model_path("tokenizer.json"), b"{}").unwrap();
        assert!(mgr.semantic_ready());

        std::fs::remove_dir_all(&dir).ok();
    }
}
