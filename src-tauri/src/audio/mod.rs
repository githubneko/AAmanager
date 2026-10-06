//! 音频处理层：解码、重采样、特征提取、log-mel（CLAP / AST）、波形峰值、播放。

pub mod ast_features;
pub mod decoder;
pub mod features;
pub mod mel;
pub mod player;
pub mod resampler;
pub mod waveform;

pub use decoder::{
    decode, decode_and_extract, probe, quick_hash, DecodedAudio, MAX_DECODE_SECONDS,
};
pub use features::{extract as extract_features, generate_descriptive_tags, generate_note};
pub use waveform::compute_peaks;
