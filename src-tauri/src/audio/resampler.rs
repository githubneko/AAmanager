//! 基于 rubato 的重采样。
//!
//! 自动标签模型（YAMNet 需要 16 kHz 单声道）与后续 CLAP（48 kHz）都要求固定采样率，
//! 这里提供单声道 f32 的高质量重采样。

use crate::error::{AppError, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// 单块处理的帧数下限（同时限制内存占用）。
const MIN_CHUNK_FRAMES: usize = 4096;

/// 把单声道 f32 采样从 `from_rate` 重采样到 `to_rate`。
///
/// 采样率相同或输入为空时原样返回。
///
/// 实现要点：`SincFixedIn` 要求每次送入的输入帧数**恰好等于**创建时声明的
/// `chunk_size`，否则会返回 `Insufficient buffer size`。因此这里把块长取成
/// `ratio` 的整数倍（保证输出也是整数帧），最后一块用静音补齐再按目标长度裁剪。
pub fn resample_mono(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    if samples.is_empty() || from_rate == to_rate || from_rate == 0 || to_rate == 0 {
        return Ok(samples.to_vec());
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };

    // 块长必须是 ratio 的整数倍（ratio = to_rate/from_rate），否则 rubato
    // 得不到整数输出帧。取 from_rate 的整数倍即可满足该条件。
    let mut chunk_size = from_rate as usize;
    while chunk_size < MIN_CHUNK_FRAMES {
        chunk_size = chunk_size.saturating_mul(2);
    }

    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_size, 1)
        .map_err(|e| AppError::Audio(format!("创建重采样器失败: {e}")))?;

    let expected = (samples.len() as f64 * ratio).round() as usize;
    let mut out: Vec<f32> = Vec::with_capacity(expected + chunk_size);
    // 预分配复用缓冲，避免每块都分配
    let mut block: Vec<f32> = vec![0.0; chunk_size];

    let mut pos = 0usize;
    while pos < samples.len() {
        let n = (samples.len() - pos).min(chunk_size);
        block[..n].copy_from_slice(&samples[pos..pos + n]);
        // 末块补零：SincFixedIn 的输出与输入等长（按 ratio 换算），
        // 补零只影响尾部若干样本，之后会被按 expected 裁剪掉。
        for b in block[n..].iter_mut() {
            *b = 0.0;
        }

        let input = vec![block.clone()];
        let processed = resampler
            .process(&input, None)
            .map_err(|e| AppError::Audio(format!("重采样失败: {e}")))?;
        if let Some(ch) = processed.first() {
            out.extend_from_slice(ch);
        }
        pos += n;
    }

    // 按目标长度裁剪（重采样会因滤波器群延迟略微偏长）
    if out.len() > expected {
        out.truncate(expected);
    }
    // 极端情况下（重采样器还没吐出足够数据）用零补齐，保证长度契约
    if out.len() < expected {
        out.resize(expected, 0.0);
    }

    Ok(out)
}

/// 为 ML 推理准备输入：重采样到 16 kHz 单声道。
pub fn to_16k_mono(samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    resample_mono(samples, sample_rate, 16_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin())
            .collect()
    }

    #[test]
    fn same_rate_is_passthrough() {
        let s = sine(440.0, 44_100, 0.1);
        let out = resample_mono(&s, 44_100, 44_100).unwrap();
        assert_eq!(out, s);
    }

    #[test]
    fn empty_input_is_handled() {
        let out = resample_mono(&[], 44_100, 16_000).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn downsample_produces_expected_length() {
        let s = sine(440.0, 48_000, 0.5);
        let out = resample_mono(&s, 48_000, 16_000).unwrap();
        let expected = 8_000;
        let diff = (out.len() as i64 - expected as i64).abs();
        assert!(diff < 64, "长度 {} 应接近 {}", out.len(), expected);
    }

    #[test]
    fn upsample_produces_expected_length() {
        let s = sine(440.0, 8_000, 0.25);
        let out = resample_mono(&s, 8_000, 16_000).unwrap();
        let diff = (out.len() as i64 - 4_000i64).abs();
        assert!(diff < 64, "长度 {} 应接近 4000", out.len());
    }

    #[test]
    fn resampling_preserves_signal_energy() {
        let s = sine(440.0, 48_000, 0.5);
        let out = resample_mono(&s, 48_000, 16_000).unwrap();
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let (a, b) = (rms(&s), rms(&out));
        assert!((a - b).abs() < 0.05, "原始 RMS {a} 重采样后 {b}");
    }

    #[test]
    fn to_16k_helper_works() {
        let s = sine(200.0, 22_050, 0.2);
        let out = to_16k_mono(&s, 22_050).unwrap();
        assert!(!out.is_empty());
        assert!((out.len() as f64 - 3_200.0).abs() < 200.0);
    }

    #[test]
    fn handles_input_longer_than_chunk() {
        // 2 秒 48k = 96000 帧，超过 CHUNK_FRAMES(65536)，走分块分支
        let s = sine(300.0, 48_000, 2.0);
        let out = resample_mono(&s, 48_000, 16_000).unwrap();
        assert!(
            (out.len() as i64 - 32_000).abs() < 200,
            "长度 {}",
            out.len()
        );
    }
}
