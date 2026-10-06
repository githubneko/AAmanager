//! 声学特征提取与自动标签规则引擎。
//!
//! 对应策划书 4.3 / 4.4 / 4.5 节。所有计算都是纯 Rust 实现，
//! 不依赖 Python，也不加载任何模型，因此可以在导入时全速运行。

use crate::models::{AcousticFeatures, Tag, TagSource};
use rustfft::{num_complex::Complex, FftPlanner};

/// FFT 帧长（样本数）。
const FRAME_SIZE: usize = 2048;
/// 帧间跳跃。
const HOP_SIZE: usize = 1024;
/// 包络分析的窗口长度（毫秒）。
const ENVELOPE_WINDOW_MS: f32 = 5.0;

/// 从单声道采样中提取声学特征。
///
/// * `duration` —— 完整时长（秒）；传 0 则用样本数推算。
pub fn extract(samples: &[f32], sample_rate: u32, duration: f64) -> AcousticFeatures {
    let sr = if sample_rate == 0 {
        44_100
    } else {
        sample_rate
    } as f32;
    let duration_s = if duration > 0.0 {
        duration as f32
    } else {
        samples.len() as f32 / sr
    };

    if samples.is_empty() {
        return AcousticFeatures {
            duration: duration_s,
            ..Default::default()
        };
    }

    let rms = compute_rms(samples);
    let zcr = compute_zcr(samples);

    let spectrum = average_spectrum(samples);
    let (centroid, rolloff, flatness) = if spectrum.is_empty() {
        (0.0, 0.0, 0.0)
    } else {
        (
            spectral_centroid(&spectrum, sr),
            spectral_rolloff(&spectrum, sr, 0.85),
            spectral_flatness(&spectrum),
        )
    };

    let envelope = compute_envelope(samples, sr);
    let (attack_time, decay_time) = compute_attack_decay(&envelope, sr);

    AcousticFeatures {
        // 超过 30 秒的音频只解码了前 30 秒，但时长应报告真实值
        duration: duration_s,
        rms,
        zcr,
        spectral_centroid: sanitize(centroid),
        spectral_rolloff: sanitize(rolloff),
        spectral_flatness: sanitize(flatness),
        attack_time: sanitize(attack_time),
        decay_time: sanitize(decay_time),
    }
}

/// 把 NaN / Inf 归一为 0，避免污染数据库与 JSON。
fn sanitize(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// 均方根能量。
pub fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    sanitize((sum / samples.len() as f32).sqrt())
}

/// 过零率（每秒过零次数 / 采样率，归一到 0-1 量级）。
///
/// 策划书原文除以样本数，得到的是「每样本过零概率」；此处沿用该定义，
/// 阈值 0.3 与文档一致。
pub fn compute_zcr(samples: &[f32]) -> f32 {
    if samples.len() < 2 {
        return 0.0;
    }
    let crossings = samples
        .windows(2)
        .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
        .count();
    sanitize(crossings as f32 / samples.len() as f32)
}

/// 计算各帧幅度谱的平均值（用于频谱特征）。
fn average_spectrum(samples: &[f32]) -> Vec<f32> {
    if samples.len() < FRAME_SIZE {
        // 太短的音频补零到一帧再算
        let mut buf = samples.to_vec();
        buf.resize(FRAME_SIZE, 0.0);
        return single_frame_spectrum(&buf);
    }

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FRAME_SIZE);
    let window = hann_window(FRAME_SIZE);

    let bins = FRAME_SIZE / 2 + 1;
    let mut acc = vec![0f32; bins];
    let mut frames = 0usize;

    let mut pos = 0usize;
    while pos + FRAME_SIZE <= samples.len() {
        let mut buf: Vec<Complex<f32>> = (0..FRAME_SIZE)
            .map(|i| Complex::new(samples[pos + i] * window[i], 0.0))
            .collect();
        fft.process(&mut buf);
        for (b, c) in buf.iter().take(bins).enumerate() {
            acc[b] += c.norm();
        }
        frames += 1;
        pos += HOP_SIZE;
    }

    if frames == 0 {
        return Vec::new();
    }
    let inv = 1.0 / frames as f32;
    acc.iter().map(|v| v * inv).collect()
}

/// 单帧幅度谱（用于短音频）。
fn single_frame_spectrum(frame: &[f32]) -> Vec<f32> {
    let window = hann_window(FRAME_SIZE);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FRAME_SIZE);
    let mut buf: Vec<Complex<f32>> = (0..FRAME_SIZE)
        .map(|i| Complex::new(frame.get(i).copied().unwrap_or(0.0) * window[i], 0.0))
        .collect();
    fft.process(&mut buf);
    buf.iter()
        .take(FRAME_SIZE / 2 + 1)
        .map(|c| c.norm())
        .collect()
}

/// Hann 窗。
fn hann_window(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f32::consts::TAU * i as f32 / (n as f32 - 1.0);
            0.5 * (1.0 - x.cos())
        })
        .collect()
}

/// 频谱质心（Hz）——「亮度」指标。
pub fn spectral_centroid(spectrum: &[f32], sample_rate: f32) -> f32 {
    let total: f32 = spectrum.iter().sum();
    if total <= f32::EPSILON {
        return 0.0;
    }
    let bin_hz = sample_rate / 2.0 / (spectrum.len().max(1) as f32 - 1.0).max(1.0);
    let weighted: f32 = spectrum
        .iter()
        .enumerate()
        .map(|(i, m)| i as f32 * bin_hz * m)
        .sum();
    weighted / total
}

/// 频谱滚降点（Hz）：累计能量达到 `ratio` 处的频率。
pub fn spectral_rolloff(spectrum: &[f32], sample_rate: f32, ratio: f32) -> f32 {
    let total: f32 = spectrum.iter().map(|m| m * m).sum();
    if total <= f32::EPSILON {
        return 0.0;
    }
    let threshold = total * ratio;
    let mut acc = 0f32;
    let bin_hz = sample_rate / 2.0 / (spectrum.len().max(1) as f32 - 1.0).max(1.0);
    for (i, m) in spectrum.iter().enumerate() {
        acc += m * m;
        if acc >= threshold {
            return i as f32 * bin_hz;
        }
    }
    (spectrum.len() as f32 - 1.0).max(0.0) * bin_hz
}

/// 频谱平坦度：几何平均 / 算术平均。接近 1 表示噪声，接近 0 表示纯音。
pub fn spectral_flatness(spectrum: &[f32]) -> f32 {
    if spectrum.is_empty() {
        return 0.0;
    }
    // 加一个小量避免 log(0)
    const EPS: f32 = 1e-10;
    let n = spectrum.len() as f32;
    let arithmetic: f32 = spectrum.iter().sum::<f32>() / n;
    if arithmetic <= EPS {
        return 0.0;
    }
    let log_sum: f32 = spectrum.iter().map(|m| (m + EPS).ln()).sum();
    let geometric = (log_sum / n).exp();
    sanitize((geometric / arithmetic).clamp(0.0, 1.0))
}

/// 计算幅度包络（每 5 ms 取一个 RMS 值）。
pub fn compute_envelope(samples: &[f32], sample_rate: f32) -> Vec<f32> {
    let win = ((sample_rate * ENVELOPE_WINDOW_MS / 1000.0) as usize).max(1);
    samples
        .chunks(win)
        .map(|c| {
            let sum: f32 = c.iter().map(|s| s * s).sum();
            (sum / c.len().max(1) as f32).sqrt()
        })
        .collect()
}

/// 由包络计算起振时间与衰减时间（秒）。
///
/// * 起振时间：从包络起点到全局峰值所需时间。
/// * 衰减时间：从峰值衰减到峰值 10%（-20 dB）所需时间。
pub fn compute_attack_decay(envelope: &[f32], sample_rate: f32) -> (f32, f32) {
    if envelope.is_empty() {
        return (0.0, 0.0);
    }
    let win_secs = (ENVELOPE_WINDOW_MS / 1000.0).max(1.0 / sample_rate);

    let (peak_idx, peak) =
        envelope.iter().enumerate().fold(
            (0usize, 0f32),
            |(bi, bv), (i, &v)| {
                if v > bv {
                    (i, v)
                } else {
                    (bi, bv)
                }
            },
        );

    if peak <= 1e-6 {
        // 近乎静音
        return (0.0, 0.0);
    }

    let attack = peak_idx as f32 * win_secs;

    let target = peak * 0.1;
    let decay = envelope
        .iter()
        .skip(peak_idx)
        .position(|&v| v <= target)
        .map(|off| off as f32 * win_secs)
        .unwrap_or(0.0);

    (sanitize(attack), sanitize(decay))
}

// ---------------------------------------------------------------------------
// 规则引擎（策划书 4.4）
// ---------------------------------------------------------------------------

/// 根据声学特征生成描述性标签。
///
/// 阈值与置信度直接采用策划书 4.4 节的取值。
pub fn generate_descriptive_tags(f: &AcousticFeatures) -> Vec<Tag> {
    let mut tags: Vec<Tag> = Vec::new();
    let mut push = |name: &str, conf: f32| {
        // 同名标签只保留置信度更高的一条
        if let Some(existing) = tags.iter_mut().find(|t| t.tag == name) {
            if conf > existing.confidence {
                existing.confidence = conf;
            }
            return;
        }
        tags.push(Tag::new(name, TagSource::Rule, conf));
    };

    // 近乎静音：频谱特征没有意义，只标记静音，避免生成「瞬态」「安静」
    // 之类的自相矛盾标签。
    if f.rms < 1e-4 {
        push("静音", 0.95);
        return tags;
    }

    // 时长
    if f.duration < 0.3 {
        push("短促", 0.9);
    } else if f.duration > 5.0 {
        push("长音", 0.8);
    } else if f.duration > 1.0 {
        push("中等时长", 0.6);
    }

    // 频谱质心 —— 「亮度」。
    //
    // 声学上更稳健的做法是把质心与滚降点结合判断。只靠质心会出现自相矛盾的
    // 结果：例如「低频为主但含有大量高频能量」的噪声（宽带撞击、爆炸），
    // 质心可能只有 3.5 kHz 却拥有 6 kHz 的滚降点，此时既不能说它尖锐也不能
    // 说它沉闷。因此：
    //   * 判为「沉闷/低频」额外要求滚降点也很低（能量确实集中在中低频）；
    //   * 判为「尖锐/高频」额外要求滚降点较高。
    let low_rolloff = f.spectral_rolloff > 0.0 && f.spectral_rolloff < 2000.0;
    let high_rolloff = f.spectral_rolloff >= 6000.0;

    if f.spectral_centroid > 3000.0 && !low_rolloff {
        push("尖锐", 0.85);
        push("高频", 0.8);
    } else if f.spectral_centroid > 0.0 && f.spectral_centroid < 500.0 && low_rolloff {
        push("沉闷", 0.85);
        push("低频", 0.8);
    }

    // 过零率
    if f.zcr > 0.3 {
        push("噪声感", 0.7);
    } else if f.zcr > 0.1 {
        push("含噪", 0.55);
    }

    // 攻击时间
    if f.attack_time < 0.01 {
        push("瞬态", 0.8);
    } else if f.attack_time > 0.2 {
        push("缓入", 0.6);
    }

    // RMS 响度
    if f.rms > 0.5 {
        push("响度大", 0.75);
    } else if f.rms < 0.02 {
        push("安静", 0.7);
    }

    // 频谱平坦度（噪声 vs 乐音）
    if f.spectral_flatness > 0.4 {
        push("宽带噪声", 0.7);
    } else if f.spectral_flatness > 0.0 && f.spectral_flatness < 0.05 {
        push("乐音感", 0.65);
    }

    // 频谱滚降：与上面的质心判断共享阈值，保证亮/暗标签互斥
    if high_rolloff {
        push("明亮", 0.7);
    } else if low_rolloff {
        push("暗淡", 0.65);
    }

    tags
}

/// 根据标签组合生成可读的中文备注（策划书 4.5）。
///
/// `label` 用于把规范标签翻译成当前词汇表的说法。例：同一个爆炸音效，
/// 游戏词汇表下生成「爆炸 / 轰鸣、宽带噪声的音频」，音乐词汇表下生成
/// 「低频厚重、宽带噪声的音频」。
pub fn generate_note_with(tags: &[Tag], label: &dyn Fn(&str) -> String) -> String {
    let descriptors: Vec<String> = tags
        .iter()
        .filter(|t| t.source == TagSource::Rule)
        .map(|t| label(&t.tag))
        .collect();

    let categories: Vec<&str> = tags
        .iter()
        .filter(|t| matches!(t.source, TagSource::Yamnet | TagSource::Clap))
        .map(|t| t.tag.as_str())
        .collect();

    let mut parts: Vec<String> = Vec::new();
    if !descriptors.is_empty() {
        parts.push(descriptors.join("、"));
    }
    if let Some(first) = categories.first() {
        parts.push(format!("{first}类"));
    }

    if parts.is_empty() {
        "未分类音频".to_string()
    } else {
        format!("{}的音频", parts.join("，"))
    }
}

/// 用规范标签生成备注（不做词汇翻译）。
pub fn generate_note(tags: &[Tag]) -> String {
    generate_note_with(tags, &|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * amp)
            .collect()
    }

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        // 简单可复现的伪随机（线性同余），避免引入 rand 依赖
        let mut state: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 8) as f32 / 8_388_608.0 - 1.0) * amp
            })
            .collect()
    }

    #[test]
    fn empty_input_is_safe() {
        let f = extract(&[], 44_100, 0.0);
        assert_eq!(f.rms, 0.0);
        assert_eq!(f.zcr, 0.0);
        assert_eq!(f.spectral_centroid, 0.0);
        assert!(f.duration.is_finite());
    }

    #[test]
    fn rms_of_full_scale_sine_is_about_0707() {
        let s = sine(440.0, 44_100, 0.2, 1.0);
        let f = extract(&s, 44_100, 0.2);
        assert!((f.rms - 0.707).abs() < 0.05, "RMS = {}", f.rms);
    }

    #[test]
    fn silence_has_zero_rms() {
        let s = vec![0.0f32; 44_100];
        let f = extract(&s, 44_100, 1.0);
        assert!(f.rms < 1e-6);
        assert_eq!(f.spectral_centroid, 0.0);
    }

    #[test]
    fn centroid_tracks_frequency() {
        let low = extract(&sine(200.0, 44_100, 0.5, 0.8), 44_100, 0.5);
        let high = extract(&sine(6000.0, 44_100, 0.5, 0.8), 44_100, 0.5);
        assert!(
            high.spectral_centroid > low.spectral_centroid * 2.0,
            "低频 {} 高频 {}",
            low.spectral_centroid,
            high.spectral_centroid
        );
        // 200 Hz 正弦的质心应接近 200
        assert!(
            (low.spectral_centroid - 200.0).abs() < 200.0,
            "{}",
            low.spectral_centroid
        );
    }

    #[test]
    fn noise_is_flatter_than_a_pure_tone() {
        let tone = extract(&sine(1000.0, 44_100, 0.3, 0.8), 44_100, 0.3);
        let n = extract(&noise(44_100, 0.8), 44_100, 1.0);
        assert!(
            n.spectral_flatness > tone.spectral_flatness,
            "噪声平坦度 {} 应大于纯音 {}",
            n.spectral_flatness,
            tone.spectral_flatness
        );
        assert!(tone.spectral_flatness < 0.2, "纯音平坦度应很低");
    }

    #[test]
    fn zcr_is_higher_for_high_frequency() {
        let low = extract(&sine(100.0, 44_100, 0.3, 0.8), 44_100, 0.3);
        let high = extract(&sine(8000.0, 44_100, 0.3, 0.8), 44_100, 0.3);
        assert!(high.zcr > low.zcr, "{} vs {}", high.zcr, low.zcr);
    }

    #[test]
    fn transient_has_short_attack() {
        // 立即起振的脉冲
        let mut s = vec![0.0f32; 4410];
        for (i, v) in s.iter_mut().enumerate().take(200) {
            *v = 1.0 - i as f32 / 200.0;
        }
        let f = extract(&s, 44_100, 0.1);
        assert!(f.attack_time < 0.01, "攻击时间 {}", f.attack_time);
    }

    #[test]
    fn slow_attack_is_detected() {
        // 200 ms 线性渐入
        let rate = 44_100u32;
        let n = (rate as f32 * 0.5) as usize;
        let s: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let env = (t / 0.2).min(1.0);
                (t * 440.0 * std::f32::consts::TAU).sin() * env * 0.9
            })
            .collect();
        let f = extract(&s, rate, 0.5);
        assert!(
            f.attack_time > 0.1,
            "攻击时间应大于 0.1s，实际 {}",
            f.attack_time
        );
    }

    #[test]
    fn features_are_finite_for_random_input() {
        let s = noise(10_000, 1.0);
        let f = extract(&s, 44_100, 0.23);
        for (name, v) in [
            ("rms", f.rms),
            ("zcr", f.zcr),
            ("centroid", f.spectral_centroid),
            ("rolloff", f.spectral_rolloff),
            ("flatness", f.spectral_flatness),
            ("attack", f.attack_time),
            ("decay", f.decay_time),
        ] {
            assert!(v.is_finite(), "{name} 不是有限值: {v}");
            assert!(v >= 0.0, "{name} 不应为负: {v}");
        }
    }

    #[test]
    fn short_audio_below_one_frame_works() {
        let s = vec![0.5f32; 100];
        let f = extract(&s, 44_100, 100.0 / 44_100.0);
        assert!(f.rms > 0.0);
        assert!(f.spectral_centroid.is_finite());
    }

    // ---------------- 规则引擎 ----------------

    #[test]
    fn short_click_gets_short_and_transient_tags() {
        let f = AcousticFeatures {
            duration: 0.1,
            rms: 0.6,
            zcr: 0.05,
            spectral_centroid: 5000.0,
            spectral_rolloff: 9000.0,
            spectral_flatness: 0.3,
            attack_time: 0.005,
            decay_time: 0.02,
        };
        let tags = generate_descriptive_tags(&f);
        let names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        assert!(names.contains(&"短促"), "{names:?}");
        assert!(names.contains(&"尖锐"), "{names:?}");
        assert!(names.contains(&"高频"), "{names:?}");
        assert!(names.contains(&"瞬态"), "{names:?}");
        assert!(names.contains(&"响度大"), "{names:?}");
        assert!(names.contains(&"明亮"), "{names:?}");
        assert!(tags.iter().all(|t| t.source == TagSource::Rule));
        assert!(tags
            .iter()
            .all(|t| t.confidence > 0.0 && t.confidence <= 1.0));
    }

    #[test]
    fn low_rumble_gets_dull_tags() {
        let f = AcousticFeatures {
            duration: 8.0,
            rms: 0.3,
            zcr: 0.02,
            spectral_centroid: 200.0,
            spectral_rolloff: 800.0,
            spectral_flatness: 0.02,
            attack_time: 0.3,
            decay_time: 2.0,
        };
        let names: Vec<String> = generate_descriptive_tags(&f)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert!(names.contains(&"长音".to_string()), "{names:?}");
        assert!(names.contains(&"沉闷".to_string()), "{names:?}");
        assert!(names.contains(&"低频".to_string()), "{names:?}");
        assert!(names.contains(&"缓入".to_string()), "{names:?}");
        assert!(names.contains(&"暗淡".to_string()), "{names:?}");
        assert!(names.contains(&"乐音感".to_string()), "{names:?}");
    }

    #[test]
    fn sharp_and_dull_tags_are_mutually_exclusive() {
        // 回归测试：宽带噪声（例如爆炸）质心不高但含有大量高频能量，
        // 曾同时被标为「尖锐」和「暗淡」，语义自相矛盾。
        let f = AcousticFeatures {
            duration: 1.6,
            rms: 0.055,
            zcr: 0.02,
            spectral_centroid: 3540.0,
            spectral_rolloff: 603.0,
            spectral_flatness: 0.141,
            attack_time: 0.305,
            decay_time: 1.0,
        };
        let names: Vec<String> = generate_descriptive_tags(&f)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert!(
            !names.contains(&"尖锐".to_string()),
            "低滚降不应判为尖锐: {names:?}"
        );
        assert!(names.contains(&"暗淡".to_string()), "{names:?}");

        // 反向：高滚降不应判为沉闷
        let bright = AcousticFeatures {
            spectral_centroid: 400.0,
            spectral_rolloff: 7000.0,
            rms: 0.2,
            duration: 0.5,
            ..Default::default()
        };
        let names: Vec<String> = generate_descriptive_tags(&bright)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert!(
            !names.contains(&"沉闷".to_string()),
            "高滚降不应判为沉闷: {names:?}"
        );
        assert!(names.contains(&"明亮".to_string()), "{names:?}");
    }

    #[test]
    fn every_real_fixture_gets_non_contradictory_tags() {
        // 用真实提取路径（而非手写特征）覆盖同一约束
        for (freq, amp, secs) in [
            (90.0f32, 0.9f32, 0.9f32),
            (440.0, 0.7, 3.0),
            (6000.0, 0.6, 0.2),
        ] {
            let s = sine(freq, 44_100, secs, amp);
            let f = extract(&s, 44_100, secs as f64);
            let names: Vec<String> = generate_descriptive_tags(&f)
                .iter()
                .map(|t| t.tag.clone())
                .collect();
            let sharp = names.contains(&"尖锐".to_string()) || names.contains(&"高频".to_string());
            let dull = names.contains(&"沉闷".to_string()) || names.contains(&"低频".to_string());
            assert!(
                !(sharp && dull),
                "freq={freq} 不应同时尖锐又沉闷: {names:?}"
            );
            let bright = names.contains(&"明亮".to_string());
            let dark = names.contains(&"暗淡".to_string());
            assert!(
                !(bright && dark),
                "freq={freq} 不应同时明亮又暗淡: {names:?}"
            );
        }
    }

    #[test]
    fn noisy_signal_gets_noise_tags() {
        let f = AcousticFeatures {
            duration: 2.0,
            rms: 0.4,
            zcr: 0.45,
            spectral_centroid: 4000.0,
            spectral_rolloff: 7000.0,
            spectral_flatness: 0.6,
            attack_time: 0.05,
            decay_time: 0.5,
        };
        let names: Vec<String> = generate_descriptive_tags(&f)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert!(names.contains(&"噪声感".to_string()), "{names:?}");
        assert!(names.contains(&"宽带噪声".to_string()), "{names:?}");
    }

    #[test]
    fn duplicate_tags_are_merged_keeping_higher_confidence() {
        // 同时满足「短促」与「中等时长」不可能，但构造一个会走同一分支的极端值
        let f = AcousticFeatures {
            duration: 0.05,
            rms: 0.9,
            zcr: 0.5,
            spectral_centroid: 9000.0,
            spectral_rolloff: 15000.0,
            spectral_flatness: 0.9,
            attack_time: 0.0,
            decay_time: 0.0,
        };
        let tags = generate_descriptive_tags(&f);
        let mut names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(before, names.len(), "不应出现重复标签");
    }

    #[test]
    fn zero_features_produce_no_false_tags() {
        let f = AcousticFeatures::default();
        let tags = generate_descriptive_tags(&f);
        let names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        // 全 0 特征等价于静音，不应生成「瞬态」「安静」这类矛盾标签
        assert_eq!(names, vec!["静音"], "{names:?}");
    }

    #[test]
    fn near_silent_audio_is_tagged_silent_only() {
        let s = vec![1e-6f32; 44_100];
        let f = extract(&s, 44_100, 1.0);
        let names: Vec<String> = generate_descriptive_tags(&f)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert_eq!(names, vec!["静音".to_string()], "{names:?}");
    }

    #[test]
    fn quiet_but_real_audio_still_gets_tags() {
        // 明显安静但并非静音的信号，应保留「安静」标签
        let s = sine(440.0, 44_100, 0.5, 0.01);
        let f = extract(&s, 44_100, 0.5);
        assert!(f.rms > 1e-4, "RMS 应大于静音阈值: {}", f.rms);
        let names: Vec<String> = generate_descriptive_tags(&f)
            .iter()
            .map(|t| t.tag.clone())
            .collect();
        assert!(names.contains(&"安静".to_string()), "{names:?}");
        assert!(!names.contains(&"静音".to_string()), "{names:?}");
    }

    // ---------------- 自动备注 ----------------

    #[test]
    fn note_combines_descriptors_and_category() {
        let tags = vec![
            Tag::new("短促", TagSource::Rule, 0.9),
            Tag::new("尖锐", TagSource::Rule, 0.85),
            Tag::new("Gunshot", TagSource::Yamnet, 0.8),
        ];
        let note = generate_note(&tags);
        assert!(note.contains("短促"), "{note}");
        assert!(note.contains("尖锐"), "{note}");
        assert!(note.contains("、"), "描述词应以顿号连接");
        assert!(note.contains("Gunshot类"), "{note}");
        assert!(note.ends_with("的音频"), "{note}");
    }

    #[test]
    fn note_defaults_when_no_tags() {
        assert_eq!(generate_note(&[]), "未分类音频");
    }

    #[test]
    fn note_ignores_user_tags_for_descriptor_part() {
        let tags = vec![Tag::new("我的素材", TagSource::User, 1.0)];
        assert_eq!(generate_note(&tags), "未分类音频");
    }

    #[test]
    fn note_uses_clap_category_as_fallback() {
        let tags = vec![Tag::new("explosion", TagSource::Clap, 0.7)];
        let note = generate_note(&tags);
        assert!(note.contains("explosion类"), "{note}");
    }
}
