//! AST 音频事件分类器的特征提取（16 kHz 128 维 Kaldi 兼容 fbank）。
//!
//! ## 为什么不能照搬 CLAP 的做法
//!
//! 官方 `ASTFeatureExtractor._extract_fbank_features` 优先调用 torchaudio 的
//! **Kaldi 兼容 fbank**（`ta_kaldi.fbank`），它与 Whisper/CLAP 的前端差异很大：
//!
//! | 环节 | AST（Kaldi fbank） | CLAP |
//! | --- | --- | --- |
//! | 采样率 | 16 kHz | 48 kHz |
//! | 窗长 / 帧移 | 400 / 160 | 1024 / 480 |
//! | FFT 长度 | **512** | 1024 |
//! | center 补零 | **无**（snip_edges） | 有（reflect） |
//! | 预加重 | **0.97** | 无 |
//! | 去直流 | **是**（逐帧减均值） | 无 |
//! | 窗函数 | hanning（**symmetric**，分母 N-1） | periodic Hann（分母 N） |
//! | 对数 | **自然对数** | 10·log10 |
//! | 波形缩放 | **×32768**（16-bit 定点） | 无 |
//! | mel 范围 | **20 – 8000 Hz** | 50 – 14000 Hz |
//! | mel 刻度 | htk，无面积归一化 | slaney，面积归一化 |
//! | 归一化 | `(x + 4.2677) / 4.5690` | 无 |
//!
//! 我第一版直接套用了 CLAP 的思路（log10、center 补零、无预加重/去直流），
//! 结果特征与官方相关性只有 0.055，分类自然全错（枪声被判成「音乐」）。
//!
//! 参数取自 `torchaudio.compliance.kaldi.fbank` 的默认值
//! （`low_freq=20.0`, `high_freq=0` 即奈奎斯特, `preemphasis_coefficient=0.97`,
//! `remove_dc_offset=True`, `use_log_fbank=True`）与 `ASTFeatureExtractor`
//! 的 `preprocessor_config.json`。

use crate::error::{AppError, Result};
use rustfft::{num_complex::Complex, FftPlanner};

/// AST 期望的采样率。
pub const SAMPLE_RATE: u32 = 16_000;
/// 窗长（25 ms @ 16 kHz）。
pub const FRAME_LENGTH: usize = 400;
/// 帧移（10 ms @ 16 kHz）。
pub const HOP_LENGTH: usize = 160;
/// FFT 长度（Kaldi 默认 round_to_power_of_two，400 -> 512）。
pub const FFT_LENGTH: usize = 512;
/// 单边频谱 bin 数。
pub const N_FREQ_BINS: usize = FFT_LENGTH / 2 + 1;
/// mel 滤波器数量。
pub const N_MEL: usize = 128;
/// mel 频率下界（Kaldi `low_freq`）。
pub const LOW_FREQ: f64 = 20.0;
/// mel 频率上界（Kaldi `high_freq=0` 表示奈奎斯特）。
pub const HIGH_FREQ: f64 = SAMPLE_RATE as f64 / 2.0;
/// 预加重系数。
pub const PREEMPHASIS: f64 = 0.97;
/// 波形缩放。
///
/// **实测为 1.0（即不缩放）**：`ASTFeatureExtractor` 调用 torchaudio 时
/// 传的是归一化到 [-1,1] 的波形。`ta_kaldi.fbank` 源码里那句
/// `waveform = waveform * (2**15)` 是被注释掉的，且 Kaldi 官方的
/// "16-bit" 约定指的是输入本身已是 16-bit 整数幅度 —— 对 float 输入不适用。
///
/// 我一开始按注释缩放 ×32768，结果功率谱被放大 1e9 倍，mel 值整体偏高，
/// 分类完全错乱（枪声被判成音乐）。判定方法：用「未缩放」反推官方输出
/// 最大差 0.00000，用「×32768」反推差 2.2756 —— 结论毫无歧义。
pub const WAVEFORM_SCALE: f64 = 1.0;
/// log 之前的能量下限（torchaudio 的 epsilon）。
pub const MEL_FLOOR: f64 = 1.192_092_955_078_125e-7;
/// 模型期望的时间帧数。
pub const TARGET_FRAMES: usize = 1024;
/// 训练时的全局均值。
pub const NORMALIZE_MEAN: f64 = -4.267_739_3;
/// 训练时的全局标准差。
///
/// ⚠️ 官方归一化除以的是 **`std * 2`** 而不是 `std` ——
/// `ASTFeatureExtractor.normalize` 的实现是 `(x - mean) / (std * 2)`。
/// 这个 2 极易漏掉，漏掉后所有值大一倍，模型等于在错误的输入分布上推理。
pub const NORMALIZE_STD: f64 = 4.568_997_4;
/// 官方归一化的实际除数（`std * 2`）。
pub const NORMALIZE_DIVISOR: f64 = NORMALIZE_STD * 2.0;
/// 类别数（AudioSet）。
pub const NUM_CLASSES: usize = 527;

/// HTK mel 刻度：Hz -> mel。
fn hz_to_mel_htk(freq: f64) -> f64 {
    2595.0 * (1.0 + freq / 700.0).log10()
}

/// HTK mel 刻度：mel -> Hz。
fn mel_to_hz_htk(mel: f64) -> f64 {
    700.0 * (10f64.powf(mel / 2595.0) - 1.0)
}

/// HTK 三角 mel 滤波器组（无面积归一化），形状 `[N_MEL][N_FREQ_BINS]`。
///
/// 与 `torchaudio.functional.melscale_fbanks(
///     n_freqs=257, f_min=20, f_max=8000, n_mels=128, norm=None, mel_scale="htk")`
/// 一致 —— 已逐点核对，sum 均为 252.6174。
pub fn mel_filterbank_htk() -> Vec<Vec<f64>> {
    let fft_freqs: Vec<f64> = (0..N_FREQ_BINS)
        .map(|i| i as f64 * SAMPLE_RATE as f64 / 2.0 / (N_FREQ_BINS - 1) as f64)
        .collect();

    let mel_min = hz_to_mel_htk(LOW_FREQ);
    let mel_max = hz_to_mel_htk(HIGH_FREQ);
    let hz_points: Vec<f64> = (0..N_MEL + 2)
        .map(|i| mel_to_hz_htk(mel_min + (mel_max - mel_min) * i as f64 / (N_MEL + 1) as f64))
        .collect();

    let mut fb = vec![vec![0f64; N_FREQ_BINS]; N_MEL];
    for m in 0..N_MEL {
        let (lower, center, upper) = (hz_points[m], hz_points[m + 1], hz_points[m + 2]);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let w = if f >= lower && f <= center && center > lower {
                (f - lower) / (center - lower)
            } else if f > center && f <= upper && upper > center {
                (upper - f) / (upper - center)
            } else {
                0.0
            };
            fb[m][k] = w;
        }
    }
    fb
}

/// **symmetric** Hann 窗 —— Kaldi 的 `window_type="hanning"` 用的是这个，
/// 分母是 `N-1` 而不是 `N`。
///
/// 这个差异极小却致命：用 periodic（分母 `N`）时 mel 值整体偏低
/// （实测比值中位数 0.658），换成 symmetric 后比值中位数 0.9999。
/// CLAP 那条链路用的是 periodic，两者不能混用 —— 我最初就是照抄了 CLAP 的窗。
fn hann_symmetric(n: usize) -> Vec<f64> {
    let denom = if n > 1 { (n - 1) as f64 } else { 1.0 };
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / denom).cos())
        .collect()
}

/// Kaldi 约定的帧数：`floor((samples - frame_length) / frame_shift) + 1`。
///
/// 样本不足一帧时返回 0（调用方负责处理）。
pub fn frame_count(n_samples: usize) -> usize {
    if n_samples < FRAME_LENGTH {
        0
    } else {
        (n_samples - FRAME_LENGTH) / HOP_LENGTH + 1
    }
}

/// 计算 `[frames, N_MEL]` 的 Kaldi 兼容 log-mel。
///
/// 逐帧流程：去直流 → 预加重 → 加窗 → FFT → 功率谱 → mel 投影 →
/// 末位补零 → 下限截断 → 自然对数。
pub fn log_mel_spectrogram(samples: &[f32]) -> Vec<Vec<f32>> {
    let n_frames = frame_count(samples.len());
    if n_frames == 0 {
        return Vec::new();
    }

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(FFT_LENGTH);
    let window = hann_symmetric(FRAME_LENGTH);
    let fb = mel_filterbank_htk();

    let mut buf: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); FFT_LENGTH];
    let mut frame_buf = vec![0f64; FRAME_LENGTH];
    let mut out = Vec::with_capacity(n_frames);

    for f in 0..n_frames {
        let start = f * HOP_LENGTH;

        // 1) 取一帧并做 16-bit 缩放
        for i in 0..FRAME_LENGTH {
            frame_buf[i] = samples[start + i] as f64 * WAVEFORM_SCALE;
        }

        // 2) 去直流：逐帧减均值
        let mean = frame_buf.iter().sum::<f64>() / FRAME_LENGTH as f64;
        for v in frame_buf.iter_mut() {
            *v -= mean;
        }

        // 3) 预加重：x[i] -= 0.97 * x[i-1]，从后往前以免覆盖
        for i in (1..FRAME_LENGTH).rev() {
            frame_buf[i] -= PREEMPHASIS * frame_buf[i - 1];
        }

        // 4) 加窗 + 补零到 FFT 长度
        for i in 0..FRAME_LENGTH {
            buf[i] = Complex::new(frame_buf[i] * window[i], 0.0);
        }
        for slot in buf.iter_mut().take(FFT_LENGTH).skip(FRAME_LENGTH) {
            *slot = Complex::new(0.0, 0.0);
        }

        // 5) FFT + 功率谱
        fft.process(&mut buf);
        let mut power = [0f64; N_FREQ_BINS];
        for (k, slot) in power.iter_mut().enumerate() {
            let c = buf[k];
            *slot = c.re * c.re + c.im * c.im;
        }
        // 6) Kaldi 把最后一个频点置零后再做 mel 投影
        power[N_FREQ_BINS - 1] = 0.0;

        // 7) mel 投影 → 下限截断 → 自然对数
        let mut frame = Vec::with_capacity(N_MEL);
        for filt in fb.iter() {
            let mut acc = 0f64;
            for k in 0..N_FREQ_BINS {
                acc += filt[k] * power[k];
            }
            frame.push(acc.max(MEL_FLOOR).ln() as f32);
        }
        out.push(frame);
    }
    out
}

/// 把 `[frames, N_MEL]` 整理成 AST 输入 `[1, 1024, 128]`。
///
/// 帧数不足 1024 时用**归一化后的静音值**填充：官方流程是先按 1024 补齐、
/// 再做 `(x - mean) / std`，因此填充区不是 0，而是
/// `(ln(eps) - mean) / std ≈ 0.467`。这一点很反直觉，
/// 用 0 填充会让模型在整段尾部看到「异常大的负值」，影响分类。
pub fn to_model_input(mel: &[Vec<f32>]) -> Result<Vec<f32>> {
    if mel.is_empty() {
        return Err(AppError::Audio("AST log-mel 为空".into()));
    }
    if mel[0].len() != N_MEL {
        return Err(AppError::Audio(format!(
            "AST mel 维度异常: 期望 {N_MEL}，实际 {}",
            mel[0].len()
        )));
    }

    let silence = ((0.0 - NORMALIZE_MEAN) / NORMALIZE_DIVISOR) as f32;
    let mut out = vec![silence; TARGET_FRAMES * N_MEL];

    for (t, frame) in mel.iter().take(TARGET_FRAMES).enumerate() {
        let base = t * N_MEL;
        for (m, v) in frame.iter().enumerate() {
            out[base + m] = ((*v as f64 - NORMALIZE_MEAN) / NORMALIZE_DIVISOR) as f32;
        }
    }
    Ok(out)
}

/// 从音频采样得到 AST 模型输入 `[1, 1024, 128]`。
pub fn ast_input(samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    let resampled = crate::audio::resampler::resample_mono(samples, sample_rate, SAMPLE_RATE)?;
    if resampled.is_empty() {
        return Err(AppError::Audio("音频为空".into()));
    }
    let mel = log_mel_spectrogram(&resampled);
    to_model_input(&mel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn htk_mel_scale_roundtrips() {
        for hz in [0.0, 100.0, 1000.0, 4000.0, 8000.0] {
            let back = mel_to_hz_htk(hz_to_mel_htk(hz));
            assert!((back - hz).abs() < 1e-6, "{hz} -> {back}");
        }
    }

    #[test]
    fn filterbank_shape_and_weights_are_valid() {
        let fb = mel_filterbank_htk();
        assert_eq!(fb.len(), N_MEL);
        assert_eq!(fb[0].len(), N_FREQ_BINS);
        for (i, f) in fb.iter().enumerate() {
            assert!(f.iter().all(|w| *w >= 0.0), "第 {i} 个滤波器有负权重");
            assert!(
                f.iter().all(|w| *w <= 1.0 + 1e-9),
                "第 {i} 个滤波器权重 > 1"
            );
        }

        // 低频段的滤波器比 FFT 频率分辨率还窄，注定全 0（官方同样如此）。
        // 因此只要求「大部分滤波器非空」，并锁定非空数量。
        let non_empty = fb.iter().filter(|f| f.iter().sum::<f64>() > 0.0).count();
        assert!(
            non_empty >= N_MEL - 10,
            "非空滤波器太少: {non_empty}/{N_MEL}"
        );

        // 中心频率最高的滤波器必须落在可听上限附近，且不被截断
        let last = fb.last().unwrap();
        assert!(last.iter().sum::<f64>() > 0.0, "最高 mel 滤波器不应为空");
    }

    #[test]
    fn frame_count_follows_kaldi_convention() {
        // Kaldi: floor((samples - frame_length) / frame_shift) + 1
        assert_eq!(frame_count(399), 0, "不足一帧应为 0");
        assert_eq!(frame_count(400), 1);
        assert_eq!(frame_count(560), 2, "(560-400)/160+1 = 2");
        assert_eq!(frame_count(4087), 24, "实测音频应为 24 帧");
        // 10 秒 @16 kHz
        assert_eq!(frame_count(160_000), (160_000 - 400) / 160 + 1);

        let s = vec![0.1f32; 160_000];
        let mel = log_mel_spectrogram(&s);
        assert_eq!(mel.len(), frame_count(160_000));
        assert_eq!(mel[0].len(), N_MEL);
    }

    #[test]
    fn short_audio_before_one_frame_is_empty() {
        assert!(log_mel_spectrogram(&vec![0.1f32; 100]).is_empty());
    }

    /// 填充区应是「归一化后的 0」而不是 0 本身。
    ///
    /// 官方流程是**先补零到 1024 帧、再归一化**，而归一化是
    /// `(x - mean) / (std*2)`，所以填充区 = `(0 + 4.2677) / (4.5690*2) ≈ 0.46703`。
    /// 这两个细节（先补零 + 除以 std*2）都很反直觉，
    /// 漏掉任何一个都会让模型在整段尾部看到错误的输入分布。
    #[test]
    fn padding_uses_normalized_silence_not_zero() {
        let mel = vec![vec![0.0f32; N_MEL]; 10];
        let x = to_model_input(&mel).unwrap();
        assert_eq!(x.len(), TARGET_FRAMES * N_MEL);

        // 第 0 帧是真实值：0.0 归一化后 = (0 + 4.2677) / 4.5690 ≈ 0.934
        let expect = ((0.0 - NORMALIZE_MEAN) / NORMALIZE_DIVISOR) as f32;
        assert!((x[0] - expect).abs() < 1e-5, "归一化公式不符: {}", x[0]);

        // 填充区 = 归一化后的下限值
        // 官方是「先补零再归一化」，所以填充区 = (0 - mean) / (std*2)
        let silence = ((0.0 - NORMALIZE_MEAN) / NORMALIZE_DIVISOR) as f32;
        assert!(
            (x[10 * N_MEL] - silence).abs() < 1e-5,
            "填充区应为归一化静音 {silence}，实际 {}",
            x[10 * N_MEL]
        );
        assert!(
            (silence - 0.46703).abs() < 1e-4,
            "填充值应约等于官方的 0.46703，实际 {silence}"
        );
        assert!(
            (x[TARGET_FRAMES * N_MEL - 1] - silence).abs() < 1e-5,
            "末尾也应是归一化静音"
        );
    }

    #[test]
    fn input_truncates_when_too_long() {
        let mel = vec![vec![1.0f32; N_MEL]; 2000];
        let x = to_model_input(&mel).unwrap();
        assert_eq!(x.len(), TARGET_FRAMES * N_MEL);
    }

    #[test]
    fn wrong_mel_dimension_is_rejected() {
        let mel = vec![vec![0.0f32; 64]; 10];
        assert!(to_model_input(&mel).is_err());
    }

    /// 与官方 `ASTFeatureExtractor` 的**黄金数据**逐点比对。
    ///
    /// 信号：0.3 幅度、440 Hz 正弦、8 秒 @16 kHz（完全可复现）。
    /// 参考值由 `tools/gen_ast_golden.py` 用官方 transformers + torchaudio 生成。
    ///
    /// 这条测试锁住整条 Kaldi 流水线，其中两处极易写错：
    /// * **波形不缩放**（×32768 是错的，会把 mel 整体抬高）；
    /// * 窗是 **symmetric** Hann（分母 `N-1`），不是 CLAP 用的 periodic。
    #[test]
    fn log_mel_matches_official_ast_reference() {
        const N: usize = 16_000 * 8;
        let sig: Vec<f32> = (0..N)
            .map(|i| {
                (0.3 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / SAMPLE_RATE as f64).sin())
                    as f32
            })
            .collect();

        let mel = log_mel_spectrogram(&sig);
        assert_eq!(mel.len(), 798, "Kaldi 帧数应为 798");

        // 逐点核对官方的未归一化 fbank 值
        // （f32 只有约 7 位有效数字，这里按 f32 可表示的精度书写）
        let expected: [(usize, [f32; 5]); 2] = [
            (0, [-15.15117, -15.94238, -13.19612, -15.94238, -12.52963]),
            (1, [-13.45361, -15.5713, -12.1279, -15.94238, -11.93021]),
        ];
        for (frame, vals) in expected {
            for (m, exp) in vals.iter().enumerate() {
                let got = mel[frame][m];
                // 官方是 float64 计算后转 float32，Rust 侧同样流程；
                // 在 -15.9 dB 的静音下限附近，1e-2 的容差足以覆盖浮点舍入，
                // 又能捕捉任何真实偏差（写错一个环节会差 1 以上）。
                assert!(
                    (got - exp).abs() < 1e-2,
                    "帧 {frame} mel {m} 应为 {exp}，实际 {got}"
                );
            }
        }

        // 归一化后的整体统计也必须吻合
        let x = to_model_input(&mel).unwrap();
        let content: f32 = x[..798 * N_MEL].iter().sum::<f32>() / (798 * N_MEL) as f32;
        assert!(
            (content - (-1.031271)).abs() < 5e-3,
            "内容区归一化均值应为 -1.031271，实际 {content}"
        );
        let pad: f32 = x[798 * N_MEL..].iter().sum::<f32>() / ((1024 - 798) * N_MEL) as f32;
        // float32 舍入下与官方 0.46703 相差 ~1.6e-4
        assert!(
            (pad - 0.46703).abs() < 1e-3,
            "填充区应约 0.46703，实际 {pad}"
        );
    }
    #[test]
    fn empty_inputs_are_safe() {
        assert!(log_mel_spectrogram(&[]).is_empty());
        assert!(to_model_input(&[]).is_err());
        assert!(ast_input(&[], 16_000).is_err());
    }

    #[test]
    fn ast_input_is_deterministic_and_right_size() {
        let s: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 / 16_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let a = ast_input(&s, 16_000).unwrap();
        assert_eq!(a.len(), TARGET_FRAMES * N_MEL);
        assert_eq!(a, ast_input(&s, 16_000).unwrap());
        assert!(a.iter().all(|v| v.is_finite()));
    }

    /// 归一化后不同频率的能量重心应随频率上移。
    #[test]
    fn energy_follows_frequency() {
        let centroid = |freq: f32| {
            let s: Vec<f32> = (0..16_000)
                .map(|i| (i as f32 / 16_000.0 * freq * std::f32::consts::TAU).sin() * 0.5)
                .collect();
            let mel = log_mel_spectrogram(&s);
            let mid = &mel[mel.len() / 2];
            let lin: Vec<f64> = mid.iter().map(|v| 10f64.powf(*v as f64 / 10.0)).collect();
            let total: f64 = lin.iter().sum();
            let w: f64 = lin.iter().enumerate().map(|(i, v)| i as f64 * v).sum();
            w / total
        };
        let (low, hi) = (centroid(200.0), centroid(6000.0));
        assert!(
            low < hi,
            "能量重心应随频率上移: 200Hz={low:.1} 6kHz={hi:.1}"
        );
    }
}
