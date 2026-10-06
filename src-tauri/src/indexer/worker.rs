//! 后台分析线程。
//!
//! 一个独立线程 + 独立 SQLite 连接，逐条处理 [`queue`] 中的任务，
//! 保证 UI 线程不被解码和 FFT 计算阻塞（策划书 2.3、4.3）。
//!
//! 线程通过 `Condvar` 唤醒而不是轮询，空闲时不占用 CPU（契合「启动 < 1 秒」
//! 与「内存占用 < 100 MB」的目标）。

use crate::audio::{decoder, features, waveform};
use crate::db::{audio_repo, notes_repo, tags_repo};
use crate::indexer::queue;
use crate::models::{AcousticFeatures, IndexStatus};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// 单次分析的结果。
#[derive(Debug, Clone)]
pub struct AnalysisOutcome {
    /// 被分析的音频 id。
    pub audio_id: i64,
    /// 提取到的声学特征。
    pub features: AcousticFeatures,
    /// 生成的标签数量。
    pub tag_count: usize,
}

/// 后台索引线程的句柄。
pub struct Worker {
    /// 共享的唤醒信号。
    signal: Arc<(Mutex<bool>, Condvar)>,
    /// 请求退出。
    shutdown: Arc<AtomicBool>,
    /// 已完成任务计数（用于 UI 进度）。
    done: Arc<AtomicU64>,
    /// 失败任务计数。
    failed: Arc<AtomicU64>,
    /// 线程句柄，drop 时 join 避免泄漏。
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// 启动后台线程。
    ///
    /// * `db_path` —— 数据库文件路径（线程内自行开连接，避免跨线程共享 `Connection`）。
    pub fn start(db_path: PathBuf) -> Self {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicU64::new(0));

        let handle = {
            let signal = signal.clone();
            let shutdown = shutdown.clone();
            let done = done.clone();
            let failed = failed.clone();

            std::thread::Builder::new()
                .name("aam-indexer".into())
                .spawn(move || {
                    run_loop(db_path, signal, shutdown, done, failed);
                })
                .ok()
        };

        Self {
            signal,
            shutdown,
            done,
            failed,
            handle,
        }
    }

    /// 通知后台线程有新任务。
    pub fn notify(&self) {
        let (lock, cvar) = &*self.signal;
        if let Ok(mut pending) = lock.lock() {
            *pending = true;
            cvar.notify_one();
        }
    }

    /// 读取实时进度。
    pub fn live_counts(&self) -> (u64, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.failed.load(Ordering::Relaxed),
        )
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.notify();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 线程主循环。
fn run_loop(
    db_path: PathBuf,
    signal: Arc<(Mutex<bool>, Condvar)>,
    shutdown: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
) {
    let conn = match crate::db::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            log::error!("索引线程无法打开数据库: {e}");
            return;
        }
    };

    // 上次异常退出留下的 running 任务重新排队
    if let Err(e) = queue::reset_stale_running(&conn) {
        log::warn!("重置残留任务失败: {e}");
    }

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        match queue::take_next_pending(&conn) {
            Ok(Some(audio_id)) => {
                if let Err(e) = queue::mark_running(&conn, audio_id) {
                    log::warn!("标记任务运行失败: {e}");
                }
                match analyze_one(&conn, audio_id) {
                    Ok(_) => {
                        let _ = queue::mark_done(&conn, audio_id);
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        log::warn!("分析音频 #{audio_id} 失败: {e}");
                        let _ = queue::mark_failed(&conn, audio_id, &e.to_string());
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                // 处理完一条立刻继续下一条，不等待信号
                continue;
            }
            Ok(None) => {
                // 队列空 —— 等待唤醒，最多 2 秒超时以便响应退出请求
                let (lock, cvar) = &*signal;
                if let Ok(mut pending) = lock.lock() {
                    if !*pending {
                        let (guard, _timeout) = cvar
                            .wait_timeout(pending, Duration::from_secs(2))
                            .unwrap_or_else(|e| e.into_inner());
                        pending = guard;
                    }
                    *pending = false;
                }
            }
            Err(e) => {
                log::error!("读取任务队列失败: {e}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// 分析单个音频：解码 → 特征 → 规则标签 → 自动备注 → 波形。
pub fn analyze_one(conn: &Connection, audio_id: i64) -> crate::error::Result<AnalysisOutcome> {
    let audio = audio_repo::get(conn, audio_id)?;
    let path = std::path::Path::new(&audio.path);

    if !path.exists() {
        return Err(crate::error::AppError::NotFound(format!(
            "文件不存在: {}",
            audio.path
        )));
    }

    // 1 + 2. 解码（已降混单声道）
    let decoded = decoder::decode(path, decoder::MAX_DECODE_SECONDS)?;

    // 3. 波形峰值
    let peaks = waveform::compute_peaks(&decoded.samples, waveform::DEFAULT_PEAK_COUNT);
    audio_repo::save_waveform(conn, audio_id, &peaks)?;

    // 同时补齐 / 修正元数据（导入阶段可能拿不到准确值）
    audio_repo::update_metadata(
        conn,
        audio_id,
        Some(decoded.duration),
        Some(decoded.sample_rate as i64),
        Some(decoded.channels as i64),
        decoded.format.as_deref(),
        decoded.bitrate.map(|b| b as i64),
        std::fs::metadata(path).ok().map(|m| m.len() as i64),
    )?;

    // 4. 声学特征
    let feats = features::extract(&decoded.samples, decoded.sample_rate, decoded.duration);

    // 5. YAMNet 推理 —— 模型按需下载，尚未接入时跳过（见 crate::ml）
    let yamnet_tags = crate::ml::yamnet::infer_if_available(conn, &decoded)?;

    // 6. 规则引擎
    let mut all_tags = features::generate_descriptive_tags(&feats);
    all_tags.extend(yamnet_tags.iter().cloned());

    // 先清理旧的自动标签（不影响用户标签），再写入新的
    tags_repo::clear_auto_tags(conn, audio_id)?;
    for t in &all_tags {
        tags_repo::add_tag(conn, audio_id, &t.tag, t.source, t.confidence)?;
    }

    // 7. 自动备注：仅在没有用户备注时写入，避免覆盖用户内容。
    //    文案按用户选择的词汇表生成，让不同职业看到的描述符合自己的语言。
    let has_user_note = !notes_repo::list(conn, audio_id)?.is_empty();
    if !has_user_note {
        let resolver = crate::utils::config::Settings::load(conn)
            .map(|s| s.tag_resolver())
            .unwrap_or_else(|_| {
                crate::tag_vocab::Resolver::new(Default::default(), Default::default())
            });
        let note = features::generate_note_with(&all_tags, &|t| resolver.label(t));
        if note != "未分类音频" {
            notes_repo::set_note(conn, audio_id, &note)?;
        }
    }

    // 8. CLAP 音频编码 —— 模型缺失时静默跳过，失败只记日志不影响标签
    if let Err(e) = crate::ml::clap_audio::embed_if_available(conn, audio_id, &decoded) {
        log::warn!("音频向量生成失败（不影响标签）: {e}");
    }

    // 9. 相似度标签继承：把「听起来很像」的素材上的用户标签传播过来。
    //    放在向量生成之后，否则本音频还没有向量可比。
    //    失败同样不影响主流程 —— 继承只是锦上添花。
    match crate::ml::inherit::inherit_for(conn, audio_id) {
        Ok(out) if out.changed() => log::info!(
            "音频 {audio_id} 继承标签 {} 个，清理 {} 个（比较 {} 个候选）",
            out.inherited,
            out.pruned,
            out.compared
        ),
        Ok(_) => {}
        Err(e) => log::warn!("标签继承失败（不影响标签）: {e}"),
    }

    Ok(AnalysisOutcome {
        audio_id,
        features: feats,
        tag_count: all_tags.len(),
    })
}

/// 读取数据库中的任务统计，并与线程内的实时计数合并。
pub fn combined_status(conn: &Connection, worker: &Worker) -> crate::error::Result<IndexStatus> {
    let mut s = queue::status(conn)?;
    let (done, failed) = worker.live_counts();
    s.done = s.done.saturating_add(done as u32);
    s.failed = s.failed.saturating_add(failed as u32);
    s.running = s.running || s.pending > 0;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use crate::models::TagSource;

    /// 生成测试 WAV。
    fn write_wav(path: &std::path::Path, rate: u32, secs: f32, freq: f32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(rate as f32 * secs) as u32 {
            let v = (i as f32 / rate as f32 * freq * std::f32::consts::TAU).sin() * 0.9;
            w.write_sample((v * i16::MAX as f32) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn temp_wav(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("aam_idx_{}_{}.wav", std::process::id(), name));
        p
    }

    fn insert_audio(conn: &Connection, path: &std::path::Path) -> i64 {
        audio_repo::insert(
            conn,
            &audio_repo::NewAudio {
                path: path.to_string_lossy().to_string(),
                filename: path.file_name().unwrap().to_string_lossy().to_string(),
                folder: path.parent().unwrap().to_string_lossy().to_string(),
                duration: None,
                sample_rate: None,
                channels: None,
                format: Some("wav".into()),
                bitrate: None,
                file_size: None,
                file_hash: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn analyze_generates_tags_note_and_waveform() {
        let conn = open_in_memory().unwrap();
        let p = temp_wav("ok");
        write_wav(&p, 44_100, 0.2, 5000.0);
        let id = insert_audio(&conn, &p);

        let out = analyze_one(&conn, id).unwrap();
        assert_eq!(out.audio_id, id);
        assert!(out.tag_count > 0, "应生成自动标签");
        assert!(out.features.rms > 0.0);

        // 元数据被补齐
        let a = audio_repo::get(&conn, id).unwrap();
        assert_eq!(a.sample_rate, Some(44_100));
        assert!(a.duration.unwrap() > 0.1);

        // 波形已写入且长度固定
        let wf = audio_repo::get_waveform(&conn, id).unwrap();
        assert_eq!(wf.len(), waveform::DEFAULT_PEAK_COUNT);
        assert!(wf.iter().any(|&v| v > 0), "波形不应全为 0");

        // 自动标签：规则引擎是**无条件可靠**的来源，必须产出。
        //
        // 这里刻意不断言「有/没有 AST 标签」：`AAM_MODELS_DIR` 是**进程级**环境变量，
        // 而 `cargo test` 默认多线程并发跑测试，别的测试可能正在 set/remove 它，
        // 于是「读它来决定期望值」本身就是竞态（我一开始就是这么写的，会偶发失败）。
        // AST 是否生效由 tests/integration.rs 在**串行**场景下单独验证。
        let tags = tags_repo::list(&conn, id).unwrap();
        assert!(
            tags.iter().any(|t| t.source == TagSource::Rule),
            "规则引擎应产出标签: {tags:?}"
        );
        let notes = notes_repo::list(&conn, id).unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].note.contains("的音频"), "{}", notes[0].note);

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn analyze_does_not_overwrite_user_note() {
        let conn = open_in_memory().unwrap();
        let p = temp_wav("usernote");
        write_wav(&p, 22_050, 0.2, 800.0);
        let id = insert_audio(&conn, &p);
        notes_repo::set_note(&conn, id, "我自己写的备注").unwrap();

        analyze_one(&conn, id).unwrap();

        let notes = notes_repo::list(&conn, id).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].note, "我自己写的备注", "自动备注不应覆盖用户备注");

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn reanalysis_keeps_user_tags() {
        let conn = open_in_memory().unwrap();
        let p = temp_wav("rerun");
        write_wav(&p, 44_100, 0.15, 3000.0);
        let id = insert_audio(&conn, &p);

        analyze_one(&conn, id).unwrap();
        tags_repo::add_tag(&conn, id, "我的枪声", TagSource::User, 1.0).unwrap();

        // 再跑一次
        analyze_one(&conn, id).unwrap();

        let tags = tags_repo::list(&conn, id).unwrap();
        assert!(
            tags.iter().any(|t| t.tag == "我的枪声"),
            "重新分析不应删除用户标签: {tags:?}"
        );
        // 自动标签不应重复累积
        let auto_count = tags.iter().filter(|t| t.source == TagSource::Rule).count();
        let unique_auto: std::collections::HashSet<_> = tags
            .iter()
            .filter(|t| t.source == TagSource::Rule)
            .map(|t| t.tag.clone())
            .collect();
        assert_eq!(auto_count, unique_auto.len(), "自动标签不应重复");

        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn missing_file_reports_error() {
        let conn = open_in_memory().unwrap();
        let p = std::path::PathBuf::from("C:/nope/missing_audio.wav");
        let id = insert_audio(&conn, &p);
        let r = analyze_one(&conn, id);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("文件不存在"));
    }

    #[test]
    fn worker_drains_queue_in_background() {
        let mut db_path = std::env::temp_dir();
        db_path.push(format!("aam_worker_{}.db", std::process::id()));
        std::fs::remove_file(&db_path).ok();

        let p1 = temp_wav("w1");
        let p2 = temp_wav("w2");
        write_wav(&p1, 44_100, 0.1, 2000.0);
        write_wav(&p2, 22_050, 0.1, 500.0);

        // 主线程建库并插入数据
        let mut conn = crate::db::open(&db_path).unwrap();
        let a = insert_audio(&conn, &p1);
        let b = insert_audio(&conn, &p2);
        queue::enqueue_many(&mut conn, &[a, b]).unwrap();
        drop(conn);

        let worker = Worker::start(db_path.clone());
        worker.notify();

        // 等待队列排空（最多 20 秒）
        let mut ok = false;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(100));
            let c = crate::db::open(&db_path).unwrap();
            if queue::status(&c).unwrap().pending == 0
                && queue::take_next_pending(&c).ok().flatten().is_none()
            {
                let tags_a = tags_repo::list(&c, a).unwrap();
                let tags_b = tags_repo::list(&c, b).unwrap();
                if !tags_a.is_empty() && !tags_b.is_empty() {
                    ok = true;
                    break;
                }
            }
        }
        drop(worker);

        assert!(ok, "后台线程应在超时前完成两个任务");

        for f in [&p1, &p2, &db_path] {
            std::fs::remove_file(f).ok();
        }
        std::fs::remove_file(db_path.with_extension("db-wal")).ok();
        std::fs::remove_file(db_path.with_extension("db-shm")).ok();
    }
}
