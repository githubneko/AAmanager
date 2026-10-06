//! CLAP 音频编码器：把音频编码为 512 维语义向量。
//!
//! ## 输入约定（已与官方逐点核对）
//!
//! * 波形：48 kHz 单声道，定长 10 秒（不足先 tile 再零填充，超出截断）；
//! * 特征：log-mel `[1, 1, 1001, 64]`，由 [`crate::audio::mel`] 生成，
//!   与官方 `ClapFeatureExtractor` 输出**最大偏差 8e-6**；
//! * 输出：`audio_embeds`，512 维，需 L2 归一化后用于余弦相似度。
//!
//! ## 曾经的坑（都已在 mel 模块修正并测试）
//!
//! 1. 用错滤波器组：官方 `rand_trunc` 路径用 `mel_filters_slaney`
//!    （sum 1.364），我却用了 htk 版（sum 290.15），差约 11 dB；
//! 2. 画蛇添足地把 64 个 mel 插值成 1001 —— 官方是直接透传，
//!    那个 1001 其实是**帧数**；
//! 3. 补齐时只 tile 不补零，尾部本该静音却在重复音频。

use crate::audio::mel;
use crate::error::{AppError, Result};
use crate::ml::inference::Inference;
use crate::ml::model_manager::ModelManager;
use rusqlite::Connection;
use std::path::Path;

/// 模型逻辑名。
pub const MODEL_NAME: &str = "clap_audio.onnx";

/// 输出向量维度。
pub const EMBEDDING_DIM: usize = 512;

/// 模型版本标识，写入 `audio_embeddings.model_version`，便于换模型后重建。
pub const MODEL_VERSION: &str = "clap-htsat-unfused-onnx-v1";

/// CLAP 音频编码器期望的采样率。
pub const TARGET_SAMPLE_RATE: u32 = mel::SAMPLE_RATE;

/// 输入张量元素数（1001 x 64）。
pub const INPUT_SAMPLES: usize = mel::TARGET_FREQ * mel::TARGET_FRAMES;

/// 对已准备好的模型输入做一次前向，返回归一化后的 512 维向量。
///
/// `features` 长度必须是 [`INPUT_SAMPLES`]。
pub fn embed_features(
    inference: &Inference,
    model_path: &Path,
    features: &[f32],
) -> Result<Vec<f32>> {
    if features.len() != INPUT_SAMPLES {
        return Err(AppError::Invalid(format!(
            "模型输入长度错误: 期望 {INPUT_SAMPLES}，实际 {}",
            features.len()
        )));
    }

    // [batch, channels, frames, mels] —— 用 (shape, Vec) 直接构造张量
    let input = ort::value::Tensor::from_array((
        [1i64, 1, mel::TARGET_FREQ as i64, mel::TARGET_FRAMES as i64],
        features.to_vec(),
    ))
    .map_err(|e| AppError::Audio(format!("构造输入张量失败: {e}")))?;

    inference.with_session(model_path, |session| {
        let outputs = session
            .run(ort::inputs!["input_features" => input])
            .map_err(|e| AppError::Audio(format!("CLAP 音频推理失败: {e}")))?;

        let (shape, data) = outputs
            .get("audio_embeds")
            .ok_or_else(|| AppError::Audio("模型输出缺少 audio_embeds".into()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Audio(format!("解析音频向量失败: {e}")))?;

        let vec: Vec<f32> = data.to_vec();
        if vec.len() != EMBEDDING_DIM {
            return Err(AppError::Audio(format!(
                "音频向量维度异常: 期望 {EMBEDDING_DIM}，实际 {}（shape {shape:?}）",
                vec.len()
            )));
        }
        Ok(normalize(vec))
    })
}

/// L2 归一化（零向量原样返回）。
pub fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// 若模型可用则为该音频生成并入库向量，返回是否成功写入。
///
/// 模型缺失时返回 `Ok(false)`；推理失败则返回错误，由调用方决定是否记录。
pub fn embed_if_available(
    conn: &Connection,
    audio_id: i64,
    audio: &crate::audio::decoder::DecodedAudio,
) -> Result<bool> {
    let mgr = ModelManager::with_configured_dir(conn);
    if !mgr.is_installed(MODEL_NAME) {
        return Ok(false);
    }
    let model_path = mgr.model_path(MODEL_NAME);

    let features = mel::clap_input(&audio.samples, audio.sample_rate)?;
    let vec = embed_features(crate::ml::inference::global(), &model_path, &features)?;
    crate::db::vector_repo::upsert_embedding(conn, audio_id, &vec, MODEL_VERSION)?;
    Ok(true)
}

/// 直接对一段采样算向量（供测试与调试，不落库）。
pub fn embed_samples(
    conn: &Connection,
    samples: &[f32],
    sample_rate: u32,
) -> Result<Option<Vec<f32>>> {
    let mgr = ModelManager::with_configured_dir(conn);
    if !mgr.is_installed(MODEL_NAME) {
        return Ok(None);
    }
    let features = mel::clap_input(samples, sample_rate)?;
    Ok(Some(embed_features(
        crate::ml::inference::global(),
        &mgr.model_path(MODEL_NAME),
        &features,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_produces_unit_length() {
        let v = normalize(vec![3.0, 4.0]);
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((len - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalize_leaves_zero_vector_alone() {
        assert_eq!(normalize(vec![0.0, 0.0]), vec![0.0, 0.0]);
    }

    #[test]
    fn wrong_input_length_is_rejected_before_inference() {
        let inf = Inference::new();
        let r = embed_features(&inf, &PathBuf::from("nope.onnx"), &[0.0; 10]);
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("模型输入长度错误"), "{msg}");
    }

    #[test]
    fn constants_match_documentation() {
        assert_eq!(EMBEDDING_DIM, 512);
        assert_eq!(INPUT_SAMPLES, 1001 * 64);
        assert_eq!(TARGET_SAMPLE_RATE, 48_000);
    }
}
