//! 机器学习层：ONNX 推理内核、模型管理、相似度检索。
//!
//! ## 分发方式
//!
//! ONNX Runtime 原生库由 `ort` 在**构建时**取回并静态链接进可执行文件，
//! 因此推理内核随安装包存在，**用户不需要另行下载**。
//! 需要用户下载的只有模型文件（体积大），由 `model_manager` + `downloader` 负责。
//!
//! ## 惰性加载
//!
//! 启动时不加载任何模型（策划书 2.3）；首次用到某个模型时才建立 ONNX 会话，
//! 之后缓存在 [`inference::Inference`] 中。所有 `*_if_available` 入口在
//! 模型缺失或推理失败时**静默降级**（返回空结果 / None），保证导入链路
//! 与关键词搜索始终可用。

pub mod clap_audio;
pub mod clap_text;
pub mod downloader;
pub mod inference;
pub mod inherit;
pub mod model_manager;
pub mod similarity;
pub mod yamnet;

pub use inference::Inference;
pub use similarity::cosine_similarity;
