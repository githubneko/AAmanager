//! 基于 rodio 的音频播放。
//!
//! 采用「一次性解码到内存 + SamplesBuffer + Sink」的方式，而不是流式 Decoder，
//! 原因有三：
//! 1. 试听场景下音频普遍很短，内存代价可接受（超过 [`MAX_PLAY_SECONDS`] 才截断）；
//! 2. 可以直接定位（`skip_duration` / 重建 sink），从而实现波形点击跳转；
//! 3. 位置上报可以用样本计数精确推算，不依赖 sink 的 elapsed（仅统计已播放时长，
//!    暂停时会出错）。

use crate::error::{AppError, Result};
use rodio::buffer::SamplesBuffer;
use rodio::source::Source;
use rodio::{OutputStream, Sink};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 试听时最长加载的秒数，避免超长素材占用大量内存。
pub const MAX_PLAY_SECONDS: usize = 600;

/// 播放状态快照。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PlaybackState {
    /// 当前播放的音频 id；未播放时为 `None`。
    pub audio_id: Option<i64>,
    /// 是否正在播放。
    pub playing: bool,
    /// 已播放秒数。
    pub position: f64,
    /// 总时长（秒）。
    pub duration: f64,
    /// 音量 0.0 - 1.0。
    pub volume: f32,
}

/// 用样本计数推算播放位置的信号源包装。
///
/// rodio 的 `Sink::get_pos()` 统计的是「已播放时长」，暂停期间会继续累加，
/// 因此这里自己数样本，保证暂停/恢复后位置准确。
struct CountingSource<S> {
    inner: S,
    counter: Arc<AtomicU64>,
    channels: u16,
    sample_rate: u32,
}

impl<S: Source<Item = f32>> Iterator for CountingSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next();
        if s.is_some() {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
        s
    }
}

impl<S: Source<Item = f32>> Source for CountingSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: Duration) -> std::result::Result<(), rodio::source::SeekError> {
        // 定位后重置计数，让位置从新的起点重新累计
        let r = self.inner.try_seek(pos);
        if r.is_ok() {
            let new_pos_samples =
                pos.as_secs_f64() * self.sample_rate as f64 * self.channels as f64;
            self.counter
                .store(new_pos_samples as u64, Ordering::Relaxed);
        }
        r
    }
}

/// 播放器状态机。
///
/// 之前是靠 `Sink::empty()` / `Sink::is_paused()` 反推状态的，结果在**暂停时
/// 出 bug**：`Sink::pause()` 会让 `empty()` 变成 `true`，于是暂停被误判为
/// 「播放结束」，`state()` 返回 `playing=false` 且 `position=duration` ——
/// 表现就是「点暂停后进度条跳到末尾、按钮状态不跟手」。
///
/// 现在显式维护状态，只在真正播完（播放中且 `empty()`）时才转入 `Finished`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayState {
    /// 未加载任何音频。
    Idle,
    /// 正在播放。
    Playing,
    /// 已暂停，位置保留。
    Paused,
    /// 已播放到结尾。
    Finished,
}

/// 播放器。内部持有输出流与当前 Sink。
pub struct Player {
    /// 输出流必须常驻，drop 后播放会立即停止。
    stream: OutputStream,
    /// 当前曲目的 Sink。
    sink: Sink,
    /// 当前曲目的 Sink。
    current_id: Option<i64>,
    /// 已加载曲目的总时长（秒）。
    duration: f64,
    /// 播放位置计数器（已播放样本数）。
    counter: Arc<AtomicU64>,
    /// 显式状态（见 [`PlayState`]）。
    state: PlayState,
    /// 用于在 seek 后重新建立 sink 的样本缓存。
    buffer: Arc<Mutex<Option<CachedTrack>>>,
    /// 音量。
    volume: f32,
    /// 已加载曲目的采样率。
    sample_rate: u32,
}

/// 缓存的曲目数据，供重新 seek 时复用。
struct CachedTrack {
    samples: Vec<f32>,
}

impl Player {
    /// 创建播放器，打开系统默认输出设备。
    pub fn new() -> Result<Self> {
        let stream = rodio::OutputStreamBuilder::open_default_stream()
            .map_err(|e| AppError::Audio(format!("无法打开音频输出设备: {e}")))?;
        let sink = Sink::connect_new(stream.mixer());
        Ok(Self {
            stream,
            sink,
            current_id: None,
            duration: 0.0,
            counter: Arc::new(AtomicU64::new(0)),
            state: PlayState::Idle,
            buffer: Arc::new(Mutex::new(None)),
            volume: 1.0,
            sample_rate: 44_100,
        })
    }

    /// 加载并立即播放指定文件。
    pub fn play_file(&mut self, path: &Path, audio_id: i64) -> Result<()> {
        let decoded = crate::audio::decoder::decode(path, MAX_PLAY_SECONDS as f64)?;
        self.load_samples(
            decoded.samples,
            decoded.sample_rate,
            1,
            audio_id,
            decoded.duration,
        )
    }

    /// 用已解码的样本加载并播放（便于复用导入阶段的解码结果）。
    pub fn load_samples(
        &mut self,
        samples: Vec<f32>,
        sample_rate: u32,
        _src_channels: u32,
        audio_id: i64,
        duration: f64,
    ) -> Result<()> {
        // 输出统一为单声道，交由系统混音到多声道设备
        self.sample_rate = sample_rate.max(1);
        self.duration = if duration > 0.0 {
            duration
        } else {
            samples.len() as f64 / self.sample_rate as f64
        };
        self.current_id = Some(audio_id);
        self.counter.store(0, Ordering::Relaxed);

        *self.buffer.lock().unwrap() = Some(CachedTrack { samples });

        self.restart_sink(0.0);
        self.state = PlayState::Playing;
        Ok(())
    }

    /// 用缓存的样本重建 sink，并从 `start_secs` 开始播放。
    ///
    /// 样本不再从参数传入：它已经在 `buffer` 里，避免每次 seek 都克隆整个
    /// 音频缓冲（长素材下这是几十 MB 的拷贝，会明显卡顿）。
    fn restart_sink(&mut self, start_secs: f64) {
        let cached = self.buffer.lock().unwrap();
        let samples: &[f32] = cached.as_ref().map(|t| t.samples.as_slice()).unwrap_or(&[]);

        let skip = (start_secs.max(0.0) * self.sample_rate as f64) as usize;
        let slice: Vec<f32> = if skip < samples.len() {
            samples[skip..].to_vec()
        } else {
            Vec::new()
        };
        drop(cached);

        // 停掉旧的 sink（Sink 停止后无法重用，必须新建）
        self.sink.stop();
        let sink = Sink::connect_new(self.stream.mixer());
        sink.set_volume(self.volume);

        self.counter.store(skip as u64, Ordering::Relaxed);

        let buf = SamplesBuffer::new(1, self.sample_rate, slice);
        let counted = CountingSource {
            inner: buf,
            counter: self.counter.clone(),
            channels: 1,
            sample_rate: self.sample_rate,
        };
        sink.append(counted);
        self.sink = sink;
    }

    /// 暂停播放。
    pub fn pause(&mut self) {
        if self.state == PlayState::Playing {
            self.sink.pause();
            self.state = PlayState::Paused;
        }
    }

    /// 从暂停处继续。
    pub fn resume(&mut self) {
        match self.state {
            // 播完之后再点播放：从头开始，符合用户直觉
            PlayState::Finished => {
                self.restart_sink(0.0);
                self.state = PlayState::Playing;
            }
            PlayState::Paused => {
                self.sink.play();
                self.state = PlayState::Playing;
            }
            _ => {}
        }
    }

    /// 停止并清空当前曲目。
    pub fn stop(&mut self) {
        self.sink.stop();
        self.current_id = None;
        self.duration = 0.0;
        self.counter.store(0, Ordering::Relaxed);
        *self.buffer.lock().unwrap() = None;
        self.state = PlayState::Idle;
        // 重建一个空 sink，便于后续播放
        let s = Sink::connect_new(self.stream.mixer());
        s.set_volume(self.volume);
        self.sink = s;
    }

    /// 跳转到指定秒数。暂停状态下跳转后仍保持暂停。
    pub fn seek(&mut self, secs: f64) -> Result<()> {
        {
            let cached = self.buffer.lock().unwrap();
            if cached.is_none() {
                return Err(AppError::Invalid("当前没有加载音频".into()));
            }
        }
        let was_paused = self.state == PlayState::Paused;
        let pos = secs.clamp(0.0, self.duration.max(0.0));

        self.restart_sink(pos);

        if was_paused {
            // 保持暂停：只移动位置，不自动开始播放
            self.sink.pause();
            self.state = PlayState::Paused;
        } else {
            self.state = PlayState::Playing;
        }
        Ok(())
    }

    /// 设置音量（0.0 - 1.0）。
    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.sink.set_volume(self.volume);
    }

    /// 当前音量。
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// 读取播放状态快照。
    ///
    /// 只在「播放中且 Sink 已空」时判定为播放结束 —— 暂停时 Sink 也是空的，
    /// 单纯用 `empty()` 会把暂停误判成结束（这正是之前进度条跳到末尾的原因）。
    pub fn state(&mut self) -> PlaybackState {
        if self.state == PlayState::Playing && self.sink.empty() {
            self.state = PlayState::Finished;
        }

        let samples_played = self.counter.load(Ordering::Relaxed);
        let raw_position = samples_played as f64 / self.sample_rate.max(1) as f64;

        let playing = matches!(self.state, PlayState::Playing);
        let position = match self.state {
            PlayState::Idle => 0.0,
            PlayState::Finished => self.duration,
            _ => raw_position.min(self.duration.max(0.0)),
        };

        PlaybackState {
            audio_id: self.current_id,
            playing,
            position,
            duration: self.duration,
            volume: self.volume,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_source_counts_samples() {
        let counter = Arc::new(AtomicU64::new(0));
        let buf = SamplesBuffer::new(1, 44_100, vec![0.0f32; 100]);
        let counted = CountingSource {
            inner: buf,
            counter: counter.clone(),
            channels: 1,
            sample_rate: 44_100,
        };
        let n = counted.count();
        assert_eq!(n, 100);
        assert_eq!(counter.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn default_state_is_idle() {
        let s = PlaybackState::default();
        assert!(s.audio_id.is_none());
        assert!(!s.playing);
        assert_eq!(s.position, 0.0);
    }

    /// 无音频设备的环境（CI）下应返回错误而不是 panic。
    #[test]
    fn player_creation_failure_is_graceful() {
        match Player::new() {
            Ok(mut p) => {
                assert_eq!(p.volume(), 1.0);
                let st = p.state();
                assert!(st.audio_id.is_none());
                assert!(!st.playing);
            }
            Err(e) => {
                // 允许无声卡环境，但错误必须是可读的
                let msg = e.to_string();
                assert!(msg.contains("音频"), "错误信息应可读: {msg}");
            }
        }
    }

    /// 造一个 2 秒 1 kHz 的测试音，供播放状态测试使用。
    fn tone(secs: f64) -> (Vec<f32>, u32) {
        let rate = 48_000u32;
        let n = (rate as f64 * secs) as usize;
        let v: Vec<f32> = (0..n)
            .map(|i| (i as f32 / rate as f32 * 1000.0 * std::f32::consts::TAU).sin() * 0.3)
            .collect();
        (v, rate)
    }

    /// **回归测试**：暂停不能被误判为「播放结束」。
    ///
    /// 这是用户报告的「点暂停后进度条跳到末尾、按钮状态不跟手」的根因：
    /// `Sink::pause()` 会让 `empty()` 变 true，旧实现据此判定 finished，
    /// 于是 `playing=false` 且 `position=duration`。
    #[test]
    fn pause_is_not_mistaken_for_finished() {
        let Ok(mut p) = Player::new() else {
            eprintln!("跳过：无音频输出设备");
            return;
        };
        let (samples, rate) = tone(2.0);
        p.load_samples(samples, rate, 1, 42, 2.0).unwrap();

        let st = p.state();
        assert_eq!(st.audio_id, Some(42));
        assert!(st.playing, "加载后应立即处于播放态");
        assert_eq!(st.duration, 2.0);

        p.pause();
        let paused = p.state();
        assert!(!paused.playing, "暂停后 playing 应为 false");
        assert_eq!(paused.audio_id, Some(42), "暂停不应清空当前曲目");
        assert!(
            paused.position < 2.0,
            "暂停后位置不应跳到总时长，实际 {}",
            paused.position
        );
        assert_eq!(paused.duration, 2.0);

        // 暂停期间位置不应继续增长
        let before = p.state().position;
        std::thread::sleep(Duration::from_millis(250));
        let after = p.state().position;
        assert!(
            (after - before).abs() < 0.05,
            "暂停期间位置不应推进：{before} -> {after}"
        );

        // 恢复后应重新进入播放态，且不清零位置
        p.resume();
        let resumed = p.state();
        assert!(resumed.playing, "恢复后 playing 应为 true");
        assert!(
            (resumed.position - after).abs() < 0.05,
            "恢复不应重置位置：{after} -> {}",
            resumed.position
        );
    }

    /// **回归测试**：暂停状态下 seek 之后必须仍然是暂停。
    #[test]
    fn seek_while_paused_keeps_paused() {
        let Ok(mut p) = Player::new() else {
            eprintln!("跳过：无音频输出设备");
            return;
        };
        let (samples, rate) = tone(3.0);
        p.load_samples(samples, rate, 1, 7, 3.0).unwrap();
        p.pause();

        p.seek(1.5).unwrap();
        let st = p.state();
        assert!(!st.playing, "暂停中 seek 之后不应自动开始播放");
        assert!(
            (st.position - 1.5).abs() < 0.08,
            "seek 后位置应接近 1.5，实际 {}",
            st.position
        );

        // 播放中 seek 应保持播放
        p.resume();
        p.seek(0.5).unwrap();
        let st2 = p.state();
        assert!(st2.playing, "播放中 seek 之后应继续播放");
        assert!((st2.position - 0.5).abs() < 0.08, "实际 {}", st2.position);
    }

    /// 停止应回到 Idle：无当前曲目、位置归零。
    #[test]
    fn stop_returns_to_idle() {
        let Ok(mut p) = Player::new() else {
            eprintln!("跳过：无音频输出设备");
            return;
        };
        let (samples, rate) = tone(1.0);
        p.load_samples(samples, rate, 1, 9, 1.0).unwrap();
        assert!(p.state().playing);

        p.stop();
        let st = p.state();
        assert!(st.audio_id.is_none());
        assert!(!st.playing);
        assert_eq!(st.position, 0.0);
        assert_eq!(st.duration, 0.0);
    }

    /// 播到结尾应自动进入 Finished，位置落在总时长上；再次 resume 会从头播。
    #[test]
    fn finishing_then_resume_restarts_from_beginning() {
        let Ok(mut p) = Player::new() else {
            eprintln!("跳过：无音频输出设备");
            return;
        };
        // 40 ms 的极短音频，确保能等到自然播完
        let (samples, rate) = tone(0.04);
        p.load_samples(samples, rate, 1, 11, 0.04).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut finished = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
            let st = p.state();
            if !st.playing {
                finished = true;
                assert!(
                    (st.position - 0.04).abs() < 0.02,
                    "播完后位置应落在时长上，实际 {}",
                    st.position
                );
                break;
            }
        }
        assert!(finished, "40ms 音频应在 3 秒内播完");

        p.resume();
        let st = p.state();
        assert!(st.playing, "播完后 resume 应重新开始播放");
        assert!(
            st.position < 0.03,
            "重新开始位置应接近 0，实际 {}",
            st.position
        );
    }

    /// 未加载任何音频时 seek 应报错而不是 panic。
    #[test]
    fn seek_without_track_errors() {
        let Ok(mut p) = Player::new() else {
            eprintln!("跳过：无音频输出设备");
            return;
        };
        assert!(p.seek(1.0).is_err());
    }
}
