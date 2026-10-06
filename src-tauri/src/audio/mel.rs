//! log-mel 特征提取（CLAP 音频塔与 AST 分类器的共同输入）。
//!
//! ## 为什么需要自己实现
//!
//! CLAP 的音频塔输入不是原始波形，而是 **log-mel 特征** `[batch, 1, 1001, 64]`。
//! 必须与训练时的特征提取完全一致，否则向量会退化（表现为「所有音频对任何
//! 提示词的相似度都差不多」）。
//!
//! ## 与官方实现的对齐要点
//!
//! 参数来自 `Xenova/clap-htsat-unfused` 的 `preprocessor_config.json`，
//! 算法逐条对照 HuggingFace transformers 的 `audio_utils.spectrogram`
//! 与 `feature_extraction_clap.py`：
//!
//! | 环节 | 官方做法 | 说明 |
//! | --- | --- | --- |
//! | 滤波器组 | `norm="slaney"`, `mel_scale="slaney"` | **不是** htk；这是之前实现错的关键 |
//! | 窗口 | `window_function(1024, "hann")`，即 **periodic** Hann | 不是 symmetric |
//! | 补零 | `center=True, pad_mode="reflect"`，两侧各 512 | |
//! | 帧数 | `1 + floor((padded - frame) / hop)` | |
//! | 功率谱 | `power = 2.0`（幅度平方） | |
//! | mel | `max(1e-10, mel_filtersᵀ · power)` | 先取最大值再取对数 |
//! | 对数 | `log_mel="dB"` → `10·log10(x)`，`top_db=None` | |
//!
//! 另有测试用「黄金数据」比对：`tests/mel_reference.rs` 用官方 `audio_utils`
//! 预处理同一段音频得到的数值逐点校验。

use crate::error::{AppError, Result};
use rustfft::{num_complex::Complex, FftPlanner};

/// CLAP / AST 共用的采样率。
pub const SAMPLE_RATE: u32 = 48_000;
/// FFT 窗口长度。
pub const N_FFT: usize = 1024;
/// 帧移。
pub const HOP_LENGTH: usize = 480;
/// mel 滤波器数量。
pub const N_MEL: usize = 64;
/// 频率下界（Hz）。
pub const FMIN: f64 = 50.0;
/// 频率上界（Hz）。
pub const FMAX: f64 = 14_000.0;
/// 单边频谱 bin 数（`n_fft / 2 + 1`）。
pub const N_FREQ_BINS: usize = N_FFT / 2 + 1;
/// 模型期望的时间帧数。
pub const TARGET_FRAMES: usize = 64;
/// 模型期望的频率维（由 64 个 mel bin 插值而来）。
pub const TARGET_FREQ: usize = 1001;
/// 单次输入的样本数（10 秒 @ 48 kHz）。
pub const CHUNK_SAMPLES: usize = 480_000;
/// mel 能量的下限（与官方 `mel_floor` 一致）。
const MEL_FLOOR: f64 = 1e-10;

// ---------------------------------------------------------------------------
// Slaney mel 刻度
// ---------------------------------------------------------------------------

/// Slaney 刻度下的 break frequency（Hz）。
const SLANEY_F_SP: f64 = 200.0 / 3.0;
/// 线性段每 mel 对应的 Hz 数。
const SLANEY_MIN_LOG_HZ: f64 = 1000.0;
/// 对数段起始点的 mel 值。
const SLANEY_MIN_LOG_MEL: f64 = 15.0;
/// 对数段的步长。
const SLANEY_LOG_STEP: f64 = 0.06875177742094912; // ln(6.4) / 27

/// Hz -> mel（Slaney）。
pub fn hz_to_mel_slaney(freq: f64) -> f64 {
    let f_min = 0.0;
    let f_sp = SLANEY_F_SP;
    let min_log_hz = SLANEY_MIN_LOG_HZ;
    let min_log_mel = (min_log_hz - f_min) / f_sp;
    let logstep = SLANEY_LOG_STEP;
    let _ = SLANEY_MIN_LOG_MEL;

    if freq >= min_log_hz {
        min_log_mel + (freq / min_log_hz).ln() / logstep
    } else {
        (freq - f_min) / f_sp
    }
}

/// mel -> Hz（Slaney）。
pub fn mel_to_hz_slaney(mel: f64) -> f64 {
    let f_min = 0.0;
    let f_sp = SLANEY_F_SP;
    let min_log_hz = SLANEY_MIN_LOG_HZ;
    let min_log_mel = (min_log_hz - f_min) / f_sp;
    let logstep = SLANEY_LOG_STEP;

    if mel >= min_log_mel {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    } else {
        f_min + f_sp * mel
    }
}

/// 构建 Slaney 归一化的三角 mel 滤波器组。
///
/// 返回 `[n_mel][n_freq_bins]`（行 = mel 滤波器）。
///
/// 与官方 `mel_filter_bank(norm="slaney", mel_scale="slaney")` 一致：
/// 在 mel 空间等距取 `n_mel + 2` 个点，转回 Hz 后构造三角窗，
/// 再按滤波器带宽做面积归一化（`enorm`）。
pub fn mel_filterbank_slaney() -> Vec<Vec<f64>> {
    let n_mels = N_MEL;
    let n_bins = N_FREQ_BINS;
    let fft_freqs: Vec<f64> = (0..n_bins)
        .map(|i| i as f64 * SAMPLE_RATE as f64 / 2.0 / (n_bins - 1) as f64)
        .collect();

    let mel_min = hz_to_mel_slaney(FMIN);
    let mel_max = hz_to_mel_slaney(FMAX);
    // mel 空间等距的 n_mels + 2 个中心点
    let mel_points: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64)
        .collect();
    let hz_points: Vec<f64> = mel_points.iter().map(|m| mel_to_hz_slaney(*m)).collect();

    // 面积归一化因子：2 / (f_{i+2} - f_i)
    let enorm: Vec<f64> = (0..n_mels)
        .map(|i| 2.0 / (hz_points[i + 2] - hz_points[i]))
        .collect();

    let mut fb = vec![vec![0f64; n_bins]; n_mels];
    for m in 0..n_mels {
        let lower = hz_points[m];
        let center = hz_points[m + 1];
        let upper = hz_points[m + 2];
        for (k, &f) in fft_freqs.iter().enumerate() {
            let mut w = 0.0;
            if f >= lower && f <= center && center > lower {
                w = (f - lower) / (center - lower);
            } else if f > center && f <= upper && upper > center {
                w = (upper - f) / (upper - center);
            }
            fb[m][k] = w * enorm[m];
        }
    }
    fb
}

/// periodic Hann 窗（与官方 `window_function(n, "hann")` 一致）。
///
/// 注意是 **periodic**：`0.5 - 0.5·cos(2πn/N)`，分母是 `N` 而不是 `N-1`。
pub fn hann_periodic(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            let x = std::f64::consts::TAU * i as f64 / n as f64;
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 频谱与 log-mel
// ---------------------------------------------------------------------------

/// 计算 log-mel 频谱，返回 `[n_frames][n_mel]`。
///
/// 对应官方 `spectrogram(..., power=2.0, mel_filters=slaney, log_mel="dB")`
/// 之后转置的结果。
pub fn log_mel_spectrogram(samples: &[f32]) -> Vec<Vec<f32>> {
    if samples.is_empty() {
        return Vec::new();
    }

    // 1. center + reflect 补零（两侧各 N_FFT/2）
    let pad = N_FFT / 2;
    let padded = reflect_pad(samples, pad);

    // 2. periodic Hann 窗
    let window = hann_periodic(N_FFT);

    // 3. 帧数：1 + floor((padded - frame) / hop)
    let n_frames = if padded.len() >= N_FFT {
        1 + (padded.len() - N_FFT) / HOP_LENGTH
    } else {
        0
    };
    if n_frames == 0 {
        return Vec::new();
    }

    // 4. FFT 逐帧
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(N_FFT);

    let mel_fb = mel_filterbank_slaney();
    let n_bins = N_FREQ_BINS;

    // 预分配复用缓冲
    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); N_FFT];
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(n_frames);

    for f in 0..n_frames {
        let start = f * HOP_LENGTH;
        for i in 0..N_FFT {
            buf[i] = Complex::new(padded[start + i] as f64 * window[i], 0.0);
        }
        fft.process(&mut buf);

        // 功率谱（单边，只取前 n_bins 个）
        let mut power = [0f64; N_FREQ_BINS];
        for (k, slot) in power.iter_mut().enumerate().take(n_bins) {
            let c = buf[k];
            *slot = c.re * c.re + c.im * c.im;
        }

        // mel 投影 + log10
        let mut frame = Vec::with_capacity(N_MEL);
        for filt in mel_fb.iter() {
            let mut acc = 0f64;
            for k in 0..n_bins {
                acc += filt[k] * power[k];
            }
            let v = acc.max(MEL_FLOOR);
            // log_mel="dB": 10 * log10(x / reference)，reference 默认 1.0
            frame.push((10.0 * v.log10()) as f32);
        }
        out.push(frame);
    }

    out
}

/// reflect 模式补零（与 numpy `pad(mode="reflect")` 一致）。
///
/// `reflect` 不重复边界样本：`[a,b,c]` 补 2 个得到 `[b,a,b,c,b]`。
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f32> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    // 样本太少时 numpy 的 reflect 会失败；退化为复制边界，保证不 panic
    if n == 1 {
        return vec![samples[0]; n + 2 * pad];
    }

    let mut out = Vec::with_capacity(n + 2 * pad);
    // 左侧：索引 -pad..0 -> 1..=pad（reflect 不含边界）
    for i in 0..pad {
        let idx = pad - i; // pad, pad-1, ..., 1
        let idx = if idx < n { idx } else { reflect_index(idx, n) };
        out.push(samples[idx]);
    }
    out.extend_from_slice(samples);
    // 右侧：索引 n-1-1 ..= n-1-pad
    for i in 0..pad {
        let raw = n as isize - 2 - i as isize;
        let idx = if raw >= 0 {
            raw as usize
        } else {
            reflect_index((-raw) as usize, n)
        };
        out.push(samples[idx.min(n - 1)]);
    }
    out
}

/// 把越界索引按 reflect（周期 2n-2）折回有效范围。
fn reflect_index(mut idx: usize, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let period = 2 * n - 2;
    idx %= period;
    if idx >= n {
        period - idx
    } else {
        idx
    }
}

/// 把 `[n_frames][n_mel]` 整理成模型输入 `[1, 1, TARGET_FREQ, TARGET_FRAMES]`。
///
/// **关键**：官方处理器的输出是 `[1, 1, 1001, 64]`，其中 1001 是**帧数**、
/// 64 是 mel 数；官方把 mel 直接透传给模型，**不做任何插值**。
/// 之前我误以为 1001 是频率维，把 64 个 mel 插值成 1001 —— 那一步纯属画蛇添足，
/// 会破坏频谱结构，是音频向量退化的第二个根因。
///
/// 这里只做「帧数对齐到 1001」：不足则在时间方向重复补齐（官方 `repeatpad` +
/// 定长 10 秒后帧数通常正好是 1001），超出则截断。
pub fn to_model_input(mel: &[Vec<f32>]) -> Result<Vec<f32>> {
    if mel.is_empty() {
        return Err(AppError::Audio("log-mel 为空，无法构造模型输入".into()));
    }
    let n_mels = mel[0].len();
    let n_frames = mel.len();
    if n_mels != N_MEL {
        return Err(AppError::Audio(format!(
            "mel 维度异常: 期望 {N_MEL}，实际 {n_mels}"
        )));
    }

    // 布局为 [1, 1, TARGET_FREQ(=帧), TARGET_FRAMES(=mel)]，与官方输出一致
    let mut out = vec![0f32; TARGET_FREQ * TARGET_FRAMES];
    for q in 0..TARGET_FREQ {
        // 帧数不足时按时间重复；足够时截断
        let frame = &mel[q % n_frames];
        let base = q * TARGET_FRAMES;
        out[base..base + TARGET_FRAMES].copy_from_slice(frame);
    }
    Ok(out)
}

/// 把波形准备成 CLAP 输入：重采样到 48 kHz、定长 10 秒、log-mel、形状转换。
///
/// 补齐策略与官方 `padding="repeatpad"` 完全一致，分两步：
///   1. 先 **tile**（`CHUNK // len` 次，整数次）；
///   2. 再把剩余不足一次的尾巴用 **零填充**。
///
/// 之前我只做 tile、没做零填充，导致尾部继续重复音频而不是静音 ——
/// 官方输出里 1001 帧中有 465 帧是 -100 dB 的静音，而我只有 23 帧，
/// 这正是两者均值差 25 dB 的来源。
pub fn clap_input(samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    let resampled = crate::audio::resampler::resample_mono(samples, sample_rate, SAMPLE_RATE)?;
    let mut wav = resampled;
    if wav.is_empty() {
        return Err(AppError::Audio("音频为空".into()));
    }
    // 超过 10 秒：截断（官方 rand_trunc 是随机裁一段，特征提取时截取前段等价可用）
    wav.truncate(CHUNK_SAMPLES);

    // 不足 10 秒：先整数次 tile，再零填充
    if wav.len() < CHUNK_SAMPLES {
        let base = wav.clone();
        let reps = CHUNK_SAMPLES / base.len();
        let mut padded = Vec::with_capacity(CHUNK_SAMPLES);
        for _ in 0..reps.max(1) {
            padded.extend_from_slice(&base);
        }
        padded.resize(CHUNK_SAMPLES, 0.0); // 剩余部分补零
        wav = padded;
    }

    let mel = log_mel_spectrogram(&wav);
    to_model_input(&mel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slaney_mel_scale_roundtrips() {
        for hz in [50.0, 100.0, 200.0, 1000.0, 2000.0, 8000.0, 14000.0] {
            let mel = hz_to_mel_slaney(hz);
            let back = mel_to_hz_slaney(mel);
            assert!(
                (back - hz).abs() < 1e-6,
                "Slaney 刻度往返失败: {hz} -> {mel} -> {back}"
            );
        }
    }

    #[test]
    fn slaney_scale_is_linear_below_1khz() {
        // Slaney 刻度在 1 kHz 以下是线性的
        let a = hz_to_mel_slaney(0.0);
        let b = hz_to_mel_slaney(500.0);
        let c = hz_to_mel_slaney(1000.0);
        assert!(
            ((b - a) - (c - b)).abs() < 1e-9,
            "1kHz 以下应等距: {a} {b} {c}"
        );
    }

    #[test]
    fn filterbank_has_expected_shape_and_coverage() {
        let fb = mel_filterbank_slaney();
        assert_eq!(fb.len(), N_MEL);
        assert_eq!(fb[0].len(), N_FREQ_BINS);

        // 每个 mel 滤波器都必须有非零权重，且权重非负
        for (i, f) in fb.iter().enumerate() {
            let sum: f64 = f.iter().sum();
            assert!(sum > 0.0, "第 {i} 个滤波器全为 0");
            assert!(f.iter().all(|w| *w >= 0.0), "第 {i} 个滤波器出现负权重");
        }

        // 覆盖范围不做「每个 bin 都被覆盖」的假设：官方滤波器组只到
        // fmax = 14 kHz，更高频段的 bin 本来就不被覆盖。
        // 精确一致性由 tests/integration.rs::log_mel_matches_official_reference
        // 用官方导出的滤波器组逐点校验（偏差 < 1e-9）；这里只锁定覆盖总数。
        let covered = (0..N_FREQ_BINS)
            .filter(|k| fb.iter().any(|f| f[*k] > 0.0))
            .count();
        assert_eq!(
            covered, 297,
            "被覆盖的频点数与官方参考不一致（官方为 297/513）"
        );
    }

    #[test]
    fn hann_periodic_differs_from_symmetric() {
        let w = hann_periodic(8);
        // periodic Hann 的首元素为 0，且 w[0] != w[n-1]
        assert!((w[0] - 0.0).abs() < 1e-12);
        assert!(
            (w[7] - w[0]).abs() > 0.1,
            "periodic Hann 的首尾不应相等（那是 symmetric）"
        );
    }

    #[test]
    fn reflect_pad_matches_numpy_semantics() {
        // numpy: np.pad([1,2,3,4], 2, mode="reflect") -> [3,2,1,2,3,4,3,2]
        let s = [1.0f32, 2.0, 3.0, 4.0];
        let p = reflect_pad(&s, 2);
        assert_eq!(p, vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
    }

    #[test]
    fn reflect_pad_handles_short_and_single_inputs() {
        let p = reflect_pad(&[5.0f32], 3);
        assert_eq!(p.len(), 7);
        assert!(p.iter().all(|v| *v == 5.0));

        let p2 = reflect_pad(&[1.0f32, 2.0], 2);
        assert_eq!(p2.len(), 6);
    }

    #[test]
    fn log_mel_has_expected_shape_and_is_finite() {
        // 0.5 秒 1 kHz 正弦
        let n = SAMPLE_RATE as usize / 2;
        let s: Vec<f32> = (0..n)
            .map(|i| (i as f32 / SAMPLE_RATE as f32 * 1000.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();

        let mel = log_mel_spectrogram(&s);
        // center 补零后帧数 = 1 + (n + 1024 - 1024)/480
        let expected_frames = 1 + n / HOP_LENGTH;
        assert_eq!(mel.len(), expected_frames, "帧数与官方公式不符");
        assert_eq!(mel[0].len(), N_MEL);
        for frame in &mel {
            for v in frame {
                assert!(v.is_finite(), "log-mel 出现非有限值: {v}");
                assert!(*v <= 200.0, "dB 值异常偏大: {v}");
            }
        }
    }

    #[test]
    fn log_mel_empty_input_is_safe() {
        assert!(log_mel_spectrogram(&[]).is_empty());
    }

    #[test]
    fn to_model_input_shape_is_correct() {
        let mel = vec![vec![1.0f32; N_MEL]; 37];
        let x = to_model_input(&mel).unwrap();
        assert_eq!(x.len(), TARGET_FREQ * TARGET_FRAMES);
        // 输入为常数，输出也应处处相等
        assert!(x.iter().all(|v| (*v - 1.0).abs() < 1e-5));
    }

    /// 布局契约：`[1, 1, 帧(1001), mel(64)]` —— 与官方处理器输出一致。
    ///
    /// 官方把 mel 直接透传给模型，不做频率插值；这里只把帧数对齐到 1001。
    #[test]
    fn to_model_input_layout_is_frame_major() {
        // 3 帧，第 0 帧全 5.0，其余全 0
        let mut mel = vec![vec![0.0f32; N_MEL]; 3];
        mel[0] = vec![5.0; N_MEL];
        let x = to_model_input(&mel).unwrap();

        // x[q * TARGET_FRAMES + m]：q = 帧序号（0..1001），m = mel 序号（0..64）
        let at = |frame: usize, m: usize| x[frame * TARGET_FRAMES + m];

        // 帧 0、3、6... 来自原始第 0 帧（值为 5）
        for f in [0usize, 3, 6, 999] {
            assert!(
                (at(f, 0) - 5.0).abs() < 1e-5,
                "帧 {f} 应来自原始第 0 帧，实际 {}",
                at(f, 0)
            );
        }
        // 帧 1、2 来自原始第 1、2 帧（值为 0）
        for f in [1usize, 2, 4, 5] {
            assert!(
                at(f, 0).abs() < 1e-5,
                "帧 {f} 应来自原始第 {}帧（值为 0）",
                f % 3
            );
        }
        // 同一帧内 64 个 mel 值应一致（本测试输入是常数）
        for m in 0..TARGET_FRAMES {
            assert!((at(0, m) - 5.0).abs() < 1e-5, "mel {m} 应同为 5.0");
        }
    }

    /// 帧数超过 1001 时应截断（不 panic、长度不变）。
    #[test]
    fn to_model_input_truncates_long_mel() {
        let mel = vec![vec![2.0f32; N_MEL]; 1500];
        let x = to_model_input(&mel).unwrap();
        assert_eq!(x.len(), TARGET_FREQ * TARGET_FRAMES);
        assert!(x.iter().all(|v| (*v - 2.0).abs() < 1e-5));
    }

    /// mel 维度不对时返回错误而不是静默产出坏数据。
    #[test]
    fn to_model_input_rejects_wrong_mel_dimension() {
        let mel = vec![vec![0.0f32; 32]; 10];
        assert!(to_model_input(&mel).is_err());
    }

    #[test]
    fn clap_input_is_deterministic_and_right_size() {
        let n = SAMPLE_RATE as usize / 4;
        let s: Vec<f32> = (0..n)
            .map(|i| (i as f32 / SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.3)
            .collect();
        let a = clap_input(&s, SAMPLE_RATE).unwrap();
        let b = clap_input(&s, SAMPLE_RATE).unwrap();
        assert_eq!(a, b, "同样输入必须得到同样的特征");
        assert_eq!(a.len(), TARGET_FREQ * TARGET_FRAMES);
    }

    #[test]
    fn clap_input_rejects_empty_audio() {
        assert!(clap_input(&[], SAMPLE_RATE).is_err());
    }

    /// 不同频率的正弦，log-mel 的能量重心应随频率上移（基本单调性）。
    #[test]
    fn log_mel_energy_follows_frequency() {
        let energy_centroid = |freq: f32| {
            let n = SAMPLE_RATE as usize / 2;
            let s: Vec<f32> = (0..n)
                .map(|i| (i as f32 / SAMPLE_RATE as f32 * freq * std::f32::consts::TAU).sin() * 0.5)
                .collect();
            let mel = log_mel_spectrogram(&s);
            // 取中段的帧，避免边界效应
            let mid = &mel[mel.len() / 3];
            // 把 dB 转回线性后求重心
            let lin: Vec<f64> = mid.iter().map(|v| 10f64.powf(*v as f64 / 10.0)).collect();
            let total: f64 = lin.iter().sum();
            let weighted: f64 = lin.iter().enumerate().map(|(i, v)| i as f64 * v).sum();
            weighted / total
        };

        let low = energy_centroid(200.0);
        let mid = energy_centroid(2000.0);
        let high = energy_centroid(10000.0);
        assert!(
            low < mid && mid < high,
            "能量重心应随频率上移: 200Hz={low:.2} 2kHz={mid:.2} 10kHz={high:.2}"
        );
    }
}
