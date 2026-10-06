//! 波形峰值提取。
//!
//! 输出固定长度的峰值序列（默认 1024 点），每点取所在窗口内样本绝对值的最大值，
//! 量化为 0-255 的单字节存入 `audio_files.waveform_data`（1 KB / 文件）。
//! 前端按「点序号 × 时长 / 点数」即可把点击位置换算成播放时间。

/// 默认峰值点数量。
pub const DEFAULT_PEAK_COUNT: usize = 1024;

/// 计算波形峰值。
///
/// * `samples` —— 单声道 f32 采样。
/// * `peaks` —— 期望输出的峰值点数量。
///
/// 返回长度恒为 `peaks` 的字节数组；输入为空时全为 0。
pub fn compute_peaks(samples: &[f32], peaks: usize) -> Vec<u8> {
    let peaks = peaks.max(1);
    if samples.is_empty() {
        return vec![0u8; peaks];
    }

    let mut out = vec![0u8; peaks];
    let window = samples.len() as f64 / peaks as f64;

    for (i, slot) in out.iter_mut().enumerate() {
        let start = (i as f64 * window) as usize;
        let end = (((i + 1) as f64 * window) as usize).min(samples.len());
        // 窗口至少覆盖一个样本，避免短音频出现空洞
        let end = end.max((start + 1).min(samples.len()));
        if start >= samples.len() {
            break;
        }
        let mut peak = 0f32;
        for &s in &samples[start..end] {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        *slot = (peak.clamp(0.0, 1.0) * 255.0).round() as u8;
    }

    out
}

/// 把峰值还原为 0.0 - 1.0 的浮点序列（供测试与导出使用）。
pub fn peaks_to_floats(peaks: &[u8]) -> Vec<f32> {
    peaks.iter().map(|p| *p as f32 / 255.0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_gives_zero_peaks() {
        let p = compute_peaks(&[], 16);
        assert_eq!(p.len(), 16);
        assert!(p.iter().all(|&v| v == 0));
    }

    #[test]
    fn output_length_always_matches_request() {
        for n in [1usize, 7, 100, 1024] {
            let samples = vec![0.5f32; 50];
            assert_eq!(compute_peaks(&samples, n).len(), n);
        }
    }

    #[test]
    fn constant_signal_maps_to_full_scale() {
        let samples = vec![0.5f32; 4096];
        let peaks = compute_peaks(&samples, 64);
        // 0.5 * 255 = 127.5 -> 128
        assert!(peaks.iter().all(|&v| v == 128), "得到 {:?}", &peaks[..4]);
    }

    #[test]
    fn captures_amplitude_envelope() {
        // 前半静音，后半满幅
        let mut samples = vec![0.0f32; 1000];
        samples.extend(vec![1.0f32; 1000]);
        let peaks = compute_peaks(&samples, 10);
        assert_eq!(peaks[0], 0, "前段应为静音");
        assert_eq!(peaks[9], 255, "后段应为满幅");
    }

    #[test]
    fn handles_fewer_samples_than_peaks() {
        let samples = vec![1.0f32, 0.0, 0.5];
        let peaks = compute_peaks(&samples, 100);
        assert_eq!(peaks.len(), 100);
        assert!(peaks.contains(&255), "至少有一个满幅点");
    }

    #[test]
    fn negative_samples_use_absolute_value() {
        let samples = vec![-1.0f32; 100];
        let peaks = compute_peaks(&samples, 10);
        assert!(peaks.iter().all(|&v| v == 255));
    }

    #[test]
    fn floats_conversion_is_inverse() {
        let peaks = vec![0u8, 128, 255];
        let f = peaks_to_floats(&peaks);
        assert!((f[0] - 0.0).abs() < 1e-6);
        assert!((f[2] - 1.0).abs() < 1e-6);
    }
}
