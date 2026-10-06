//! 基于 Symphonia 的音频解码。
//!
//! 负责把任意受支持格式解成 f32 PCM，并给出时长 / 采样率 / 声道 / 码率等元数据。
//! 解码时做**降混单声道**并按需重采样。

use crate::error::{AppError, Result};
use crate::models::AcousticFeatures;
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// 解码结果。
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// 单声道 f32 采样（可能被截断，见 [`MAX_DECODE_SECONDS`]）。
    pub samples: Vec<f32>,
    /// 实际采样率（Hz）。
    pub sample_rate: u32,
    /// 源文件声道数。
    pub channels: u32,
    /// 完整时长（秒，来自容器；取不到则由已解码样本推算）。
    pub duration: f64,
    /// 码率（bit/s，可能为 `None`）。
    pub bitrate: Option<u64>,
    /// 容器短名，例如 `wav`、`mp3`。
    pub format: Option<String>,
    /// 是否因为过长而截断解码。
    pub truncated: bool,
}

/// 单次解码的时长上限。超过部分不再解码，避免长音频拖慢导入。
pub const MAX_DECODE_SECONDS: f64 = 30.0;

/// 探测结果：`(duration, sample_rate, channels, bitrate, format)`。
pub type ProbeInfo = (f64, u32, u32, Option<u64>, Option<String>);

/// 探测音频元数据，不做完整解码。
///
/// 返回 [`ProbeInfo`]。
pub fn probe(path: &Path) -> Result<ProbeInfo> {
    let d = decode(path, MAX_DECODE_SECONDS)?;
    Ok((d.duration, d.sample_rate, d.channels, d.bitrate, d.format))
}

/// 打开文件并构造 Symphonia 的格式读取器。
fn open_reader(
    path: &Path,
) -> Result<(
    Box<dyn symphonia::core::formats::FormatReader>,
    Box<dyn symphonia::core::codecs::Decoder>,
)> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // 用 Symphonia 默认注册表（symphonia 的 `all` feature 已注册全部格式与编解码器）
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| AppError::Audio(format!("无法识别格式 {}: {e}", path.display())))?;

    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AppError::Audio(format!("找不到可用音轨: {}", path.display())))?;

    // 先克隆参数，避免后续可变借用 format 时与 track 的借用冲突
    let codec_params = track.codec_params.clone();
    let decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| AppError::Audio(format!("不支持的编码: {e}")))?;

    Ok((format, decoder))
}

/// 解码音频为单声道 f32，最多解码 `max_seconds` 秒。
pub fn decode(path: &Path, max_seconds: f64) -> Result<DecodedAudio> {
    let (mut format, mut decoder) = open_reader(path)?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    let mut channels: u32 = 0;
    let mut duration_from_container: Option<f64> = None;
    let mut truncated = false;

    // 逐包解码
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // 正常结束
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(e) => return Err(AppError::Audio(format!("读取数据包失败: {e}"))),
        };

        // 首包处收集轨道信息
        if sample_rate == 0 {
            if let Some(track) = format.tracks().iter().find(|t| t.id == packet.track_id()) {
                let params = &track.codec_params;
                sample_rate = params.sample_rate.unwrap_or(0);
                channels = params.channels.map(|c| c.count() as u32).unwrap_or(1);
                // 码率优先由「文件大小 / 时长」在下方统一估算
                duration_from_container = params
                    .time_base
                    .zip(params.n_frames)
                    .map(|(tb, n)| tb.calc_time(n).seconds as f64 + tb.calc_time(n).frac);
            }
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // 单个包损坏不应中断整次导入
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(AppError::Audio(format!("解码失败: {e}"))),
        };

        if sample_rate == 0 {
            let spec = decoded.spec();
            sample_rate = spec.rate;
            channels = spec.channels.count() as u32;
        }

        downmix_into(&decoded, &mut samples);

        let rate = if sample_rate == 0 {
            48_000
        } else {
            sample_rate
        };
        if samples.len() as f64 / rate as f64 >= max_seconds {
            truncated = true;
            break;
        }
    }

    // 容器未给时长时用已解码样本推算
    let duration = duration_from_container
        .filter(|d| *d > 0.0)
        .unwrap_or_else(|| {
            if sample_rate == 0 {
                0.0
            } else {
                samples.len() as f64 / sample_rate as f64
            }
        });

    // 码率：由「文件大小 / 时长」估算。Symphonia 的 CodecParameters 没有
    // 统一的码率字段，而容器大小与时长总是可得，估算值足够展示与筛选。
    let bitrate = {
        let size = std::fs::metadata(path).ok().map(|m| m.len());
        match (size, duration) {
            (Some(size), d) if d > 0.0 => Some(((size as f64 * 8.0) / d) as u64),
            _ => None,
        }
    };

    if sample_rate == 0 {
        return Err(AppError::Audio(format!(
            "未能解码出任何音频数据: {}",
            path.display()
        )));
    }

    Ok(DecodedAudio {
        samples,
        sample_rate,
        channels: channels.max(1),
        duration,
        bitrate,
        format: path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase()),
        truncated,
    })
}

/// 把任意声道布局的音频缓冲降混为单声道并追加到 `out`。
fn downmix_into(buf: &AudioBufferRef<'_>, out: &mut Vec<f32>) {
    let spec = buf.spec();
    let ch = spec.channels.count();
    let frames = buf.frames();
    let start = out.len();
    out.resize(start + frames, 0.0);

    match buf {
        AudioBufferRef::F32(b) => {
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += *s;
                }
            }
        }
        AudioBufferRef::F64(b) => {
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += *s as f32;
                }
            }
        }
        AudioBufferRef::S32(b) => {
            // i32 满量程归一化到 [-1, 1)
            const SCALE: f32 = 1.0 / 2_147_483_648.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += *s as f32 * SCALE;
                }
            }
        }
        AudioBufferRef::S16(b) => {
            const SCALE: f32 = 1.0 / 32_768.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += *s as f32 * SCALE;
                }
            }
        }
        AudioBufferRef::S24(b) => {
            const SCALE: f32 = 1.0 / 8_388_608.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += s.inner() as f32 * SCALE;
                }
            }
        }
        AudioBufferRef::U8(b) => {
            // 无符号 8 位：128 为静音中点
            const SCALE: f32 = 1.0 / 128.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += (*s as f32 - 128.0) * SCALE;
                }
            }
        }
        AudioBufferRef::U16(b) => {
            const SCALE: f32 = 1.0 / 32_768.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += (*s as f32 - 32_768.0) * SCALE;
                }
            }
        }
        AudioBufferRef::U24(b) => {
            const SCALE: f32 = 1.0 / 8_388_608.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += (s.inner() as f32 - 8_388_608.0) * SCALE;
                }
            }
        }
        AudioBufferRef::U32(b) => {
            const SCALE: f32 = 1.0 / 2_147_483_648.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += (*s as f32 - 2_147_483_648.0) * SCALE;
                }
            }
        }
        AudioBufferRef::S8(b) => {
            const SCALE: f32 = 1.0 / 128.0;
            for c in 0..ch {
                let src = b.chan(c);
                for (i, s) in src.iter().enumerate().take(frames) {
                    out[start + i] += *s as f32 * SCALE;
                }
            }
        }
    }

    if ch > 1 {
        let inv = 1.0 / ch as f32;
        for s in &mut out[start..] {
            *s *= inv;
        }
    }
}

/// 计算内容哈希（BLAKE3，读取前 256 KB 即可有效区分音频文件，避免整文件读取）。
pub fn quick_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 256 * 1024];
    let n = file.read(&mut buf)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&buf[..n]);
    // 把文件大小一起纳入，防止「前 256KB 相同」的误判
    hasher.update(&size.to_le_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

/// 便捷函数：解码并直接抽取声学特征。
pub fn decode_and_extract(path: &Path) -> Result<(DecodedAudio, AcousticFeatures)> {
    let decoded = decode(path, MAX_DECODE_SECONDS)?;
    let features =
        crate::audio::features::extract(&decoded.samples, decoded.sample_rate, decoded.duration);
    Ok((decoded, features))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 生成一个测试用 WAV 文件（16bit 单声道）。
    fn write_wav(path: &Path, sample_rate: u32, secs: f32, freq: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let total = (sample_rate as f32 * secs) as u32;
        for i in 0..total {
            let t = i as f32 / sample_rate as f32;
            let v = (t * freq * std::f32::consts::TAU).sin() * 0.8;
            w.write_sample((v * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_test_{}_{}.wav", std::process::id(), name));
        p
    }

    #[test]
    fn decodes_wav_metadata_and_samples() {
        let p = temp_path("meta");
        write_wav(&p, 44_100, 1.0, 440.0);

        let d = decode(&p, MAX_DECODE_SECONDS).unwrap();
        assert_eq!(d.sample_rate, 44_100);
        assert_eq!(d.channels, 1);
        assert!(
            (d.duration - 1.0).abs() < 0.05,
            "时长应约为 1 秒，实际 {}",
            d.duration
        );
        assert_eq!(d.samples.len(), 44_100);
        assert!(d.bitrate.is_some());
        assert_eq!(d.format.as_deref(), Some("wav"));

        // 采样值应落在 [-1, 1]
        assert!(d.samples.iter().all(|s| s.abs() <= 1.0));

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn stereo_is_downmixed_to_mono() {
        let p = temp_path("stereo");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for _ in 0..4_800 {
            w.write_sample(16_000i16).unwrap();
            w.write_sample(-16_000i16).unwrap();
        }
        w.finalize().unwrap();

        let d = decode(&p, MAX_DECODE_SECONDS).unwrap();
        assert_eq!(d.channels, 2, "应报告源文件声道数");
        assert_eq!(d.samples.len(), 4_800, "降混后帧数等于原帧数");
        // 左右 +0.488 / -0.488 平均后接近 0
        assert!(d.samples.iter().all(|s| s.abs() < 0.01));

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn truncates_long_files() {
        let p = temp_path("long");
        // 3 秒文件，限制只解码 0.5 秒
        write_wav(&p, 8_000, 3.0, 220.0);
        let d = decode(&p, 0.5).unwrap();
        assert!(d.truncated);
        assert!(d.samples.len() <= 8_001, "最多略多于 0.5 秒");
        // 时长仍应报告完整长度
        assert!((d.duration - 3.0).abs() < 0.1, "时长 {}", d.duration);

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn probe_and_hash_work() {
        let p = temp_path("probe");
        write_wav(&p, 16_000, 0.25, 1000.0);

        let (dur, sr, ch, br, fmt) = probe(&p).unwrap();
        assert!((dur - 0.25).abs() < 0.05);
        assert_eq!(sr, 16_000);
        assert_eq!(ch, 1);
        assert!(br.is_some());
        assert_eq!(fmt.as_deref(), Some("wav"));

        let h1 = quick_hash(&p).unwrap();
        assert_eq!(h1.len(), 64, "BLAKE3 hex 长度应为 64");
        assert_eq!(h1, quick_hash(&p).unwrap(), "同一文件哈希应稳定");

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn missing_file_returns_error() {
        let p = PathBuf::from("C:/definitely/not/here/nope.wav");
        assert!(decode(&p, 1.0).is_err());
    }

    #[test]
    fn non_audio_file_returns_error() {
        let p = temp_path("junk");
        std::fs::write(&p, b"this is definitely not audio data at all").unwrap();
        let r = decode(&p, 1.0);
        assert!(r.is_err(), "非音频文件应报错而不是 panic");
        std::fs::remove_file(&p).ok();
    }
}
