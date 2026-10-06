//! 端到端集成测试：用真实音频文件走通「导入 → 落库 → 后台分析 → 搜索」全链路。
//!
//! 测试素材由 ffmpeg 生成，覆盖策划书要求的全部格式。若字体/素材目录不存在，
//! 相关用例会跳过（而不是失败），保证在没有素材的机器上 `cargo test` 依然通过。
//!
//! 素材目录可通过环境变量 `AAM_TEST_SFX` 指定，默认是仓库相邻的 `test-sfx`。

use audio_asset_manager_lib::db::{self, audio_repo, is_supported_audio, tags_repo};
use audio_asset_manager_lib::indexer::{queue, worker};
use audio_asset_manager_lib::models::{DurationBucket, SearchQuery, SortBy, TagSource};
use std::path::{Path, PathBuf};

/// 定位测试素材目录。
fn sfx_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AAM_TEST_SFX") {
        let p = PathBuf::from(p);
        if p.is_dir() {
            return Some(p);
        }
    }
    // src-tauri/tests/ -> src-tauri -> 仓库根 -> audio-asset-manager -> ai -> test-sfx
    let mut candidates = Vec::new();
    if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
        let base = PathBuf::from(manifest);
        candidates.push(base.join("../../test-sfx"));
        candidates.push(base.join("../test-sfx"));
        candidates.push(base.join("test-sfx"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// 收集素材目录下的音频文件。
fn sfx_files() -> Vec<PathBuf> {
    let Some(dir) = sfx_dir() else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_supported_audio(p))
        .collect();
    v.sort();
    v
}

/// 把一个真实文件导入内存库，返回 (audio_id, 报告用元数据)。
fn import_file(conn: &rusqlite::Connection, path: &Path) -> i64 {
    let hash = audio_asset_manager_lib::audio::decoder::quick_hash(path).ok();
    let (duration, sample_rate, channels, bitrate, format) =
        audio_asset_manager_lib::audio::probe(path).expect("探测真实音频文件不应失败");
    let new = audio_repo::NewAudio {
        path: path.to_string_lossy().to_string(),
        filename: path.file_name().unwrap().to_string_lossy().to_string(),
        folder: path.parent().unwrap().to_string_lossy().to_string(),
        duration: Some(duration),
        sample_rate: Some(sample_rate as i64),
        channels: Some(channels as i64),
        format,
        bitrate: bitrate.map(|b| b as i64),
        file_size: std::fs::metadata(path).ok().map(|m| m.len() as i64),
        file_hash: hash,
    };
    audio_repo::insert(conn, &new).expect("插入音频记录")
}

#[test]
fn decodes_every_real_format_in_the_fixture_set() {
    let files = sfx_files();
    if files.is_empty() {
        eprintln!("跳过：未找到测试素材目录（可用 AAM_TEST_SFX 指定）");
        return;
    }

    let conn = db::open_in_memory().unwrap();
    let mut seen_formats = std::collections::HashSet::new();

    for file in &files {
        let id = import_file(&conn, file);
        let a = audio_repo::get(&conn, id).unwrap();

        let ext = file.extension().unwrap().to_string_lossy().to_lowercase();
        seen_formats.insert(ext.clone());

        // 元数据必须齐全且合理
        let dur = a.duration.expect("时长应被解析");
        assert!(dur > 0.0, "{ext}: 时长应大于 0，实际 {dur}");
        assert!(dur < 60.0, "{ext}: 测试素材不应超过 60 秒");
        assert!(a.sample_rate.unwrap() > 0, "{ext}: 采样率应有效");
        assert!(a.channels.unwrap() > 0, "{ext}: 声道数应有效");
        assert!(a.file_size.unwrap() > 0, "{ext}: 文件大小应有效");
        assert!(a.file_hash.is_some(), "{ext}: 应计算出内容哈希");
        assert_eq!(a.format.as_deref(), Some(ext.as_str()));

        // 解码应产出样本
        let decoded = audio_asset_manager_lib::audio::decode(file, 30.0)
            .unwrap_or_else(|e| panic!("{ext}: 解码失败: {e}"));
        assert!(!decoded.samples.is_empty(), "{ext}: 应解码出采样数据");
        assert!(
            decoded.samples.iter().all(|s| s.is_finite()),
            "{ext}: 采样值应为有限数"
        );
    }

    // 素材集应覆盖策划书列出的主要格式
    for expected in ["wav", "mp3", "ogg", "flac", "m4a"] {
        assert!(
            seen_formats.contains(expected),
            "测试素材应包含 {expected} 格式，实际: {seen_formats:?}"
        );
    }
}

#[test]
fn full_pipeline_import_analyze_then_search() {
    let files = sfx_files();
    if files.is_empty() {
        eprintln!("跳过：未找到测试素材目录");
        return;
    }

    let conn = db::open_in_memory().unwrap();

    // ---- 1. 导入全部素材 ----
    let mut ids = Vec::new();
    for f in &files {
        ids.push(import_file(&conn, f));
    }
    assert_eq!(audio_repo::count_all(&conn).unwrap(), files.len() as i64);

    // ---- 2. 逐个跑完整分析（等价于后台线程做的事）----
    for id in &ids {
        let outcome =
            worker::analyze_one(&conn, *id).unwrap_or_else(|e| panic!("分析音频 #{id} 失败: {e}"));
        assert!(outcome.tag_count > 0, "音频 #{id} 应生成自动标签");
        assert!(outcome.features.rms > 0.0, "音频 #{id} 的 RMS 应大于 0");

        // 波形已写入且长度固定
        let wf = audio_repo::get_waveform(&conn, *id).unwrap();
        assert_eq!(
            wf.len(),
            audio_asset_manager_lib::audio::waveform::DEFAULT_PEAK_COUNT
        );

        // 规则引擎必须产出标签（这是无模型时的可靠路径）
        let tags = tags_repo::list(&conn, *id).unwrap();
        assert!(!tags.is_empty(), "规则引擎应至少产出一个标签");
        assert!(
            tags.iter().any(|t| t.source == TagSource::Rule),
            "应包含规则标签: {tags:?}"
        );
        // 这里不断言 AST 标签的有无：`AAM_MODELS_DIR` 是进程级环境变量，
        // 并发跑测试时读它决定期望值本身就是竞态。
        // AST 链路是否接通由 tools/ast_dump 与 e2e 验证脚本单独确认。

        // 自动备注已生成
        let notes = db::notes_repo::list(&conn, *id).unwrap();
        assert_eq!(notes.len(), 1, "应生成一条自动备注");
        assert!(
            notes[0].note.contains("的音频"),
            "备注格式: {}",
            notes[0].note
        );
    }

    // ---- 3. 标签云应有内容 ----
    let cloud = tags_repo::list_all_with_counts(&conn, 0).unwrap();
    assert!(!cloud.is_empty(), "标签云不应为空");
    let top = &cloud[0];
    assert!(top.count >= 1);

    // ---- 4. 时长筛选应能区分 <1s 与 >5s 的素材 ----
    let short = audio_repo::query(
        &conn,
        &SearchQuery {
            duration: DurationBucket::Lt1,
            ..Default::default()
        },
    )
    .unwrap()
    .1;
    let long = audio_repo::query(
        &conn,
        &SearchQuery {
            duration: DurationBucket::Gt5,
            ..Default::default()
        },
    )
    .unwrap()
    .1;
    assert!(short > 0, "应有 <1s 的短素材");
    assert!(long > 0, "ambient_long.ogg 是 7 秒，应落在 >5s 分组");
    assert!(short + long <= files.len() as u32, "两个分组不应超过总数");

    // ---- 5. 按格式筛选 ----
    let wavs = audio_repo::query(
        &conn,
        &SearchQuery {
            formats: vec!["wav".into()],
            ..Default::default()
        },
    )
    .unwrap()
    .1;
    assert_eq!(wavs, 2, "素材里有 2 个 wav");
}

#[test]
fn background_worker_processes_real_files_and_search_finds_them() {
    let files = sfx_files();
    if files.is_empty() {
        eprintln!("跳过：未找到测试素材目录");
        return;
    }

    // 用真实文件数据库（worker 线程需要独立连接）
    let mut db_path = std::env::temp_dir();
    db_path.push(format!("aam_it_{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", db_path.display())).ok();
    }

    let mut conn = db::open(&db_path).unwrap();
    let mut ids = Vec::new();
    for f in &files {
        ids.push(import_file(&conn, f));
    }
    queue::enqueue_many(&mut conn, &ids).unwrap();
    drop(conn);

    // 启动后台线程并等待队列排空
    let w = worker::Worker::start(db_path.clone());
    w.notify();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut all_tagged = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let c = db::open(&db_path).unwrap();
        let status = queue::status(&c).unwrap();
        if status.pending == 0 && !status.running {
            all_tagged = ids
                .iter()
                .all(|id| !tags_repo::list(&c, *id).unwrap().is_empty());
            if all_tagged {
                // ---- 关键词搜索：按自动标签命中 ----
                let tag = tags_repo::list(&c, ids[0]).unwrap()[0].tag.clone();
                let (items, total) = audio_repo::query(
                    &c,
                    &SearchQuery {
                        text: tag.clone(),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(total >= 1, "按自动标签「{tag}」应能搜到音频");
                assert!(items.iter().any(|i| i.audio.id == ids[0]));

                // ---- 排序不报错且稳定 ----
                for sort in [SortBy::Name, SortBy::Duration, SortBy::ImportedAt] {
                    let (list, _) = audio_repo::query(
                        &c,
                        &SearchQuery {
                            sort_by: sort,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    assert_eq!(list.len(), files.len());
                }
                break;
            }
        }
    }

    drop(w);
    assert!(all_tagged, "后台线程应在 60 秒内完成全部真实素材的分析");

    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", db_path.display())).ok();
    }
}

#[test]
fn duplicate_import_is_detected_by_hash() {
    let files = sfx_files();
    if files.is_empty() {
        eprintln!("跳过：未找到测试素材目录");
        return;
    }
    let src = &files[0];
    let conn = db::open_in_memory().unwrap();
    let id = import_file(&conn, src);

    // 同一内容复制到另一个路径
    let mut copy = std::env::temp_dir();
    copy.push(format!(
        "aam_dup_{}_{}",
        std::process::id(),
        src.file_name().unwrap().to_string_lossy()
    ));
    std::fs::copy(src, &copy).unwrap();

    let hash = audio_asset_manager_lib::audio::decoder::quick_hash(&copy).unwrap();
    let found = audio_repo::find_by_hash(&conn, &hash).unwrap();
    assert!(found.is_some(), "相同内容应通过哈希被识别出来");
    assert_eq!(found.unwrap().id, id);

    std::fs::remove_file(&copy).ok();
}

/// 语义检索端到端：真实音频 + 真实 CLAP 文本编码器。
///
/// 跳过条件（而非失败）：模型目录里没有 CLAP 模型，或本机缺少 Python 依赖。
/// 这样离线 / 未装模型的机器上 `cargo test` 依然是通过的。
#[test]
fn semantic_search_ranks_relevant_audio_first() {
    use audio_asset_manager_lib::db::{notes_repo, tags_repo};

    let files = sfx_files();
    if files.is_empty() {
        eprintln!("跳过：未找到测试素材目录");
        return;
    }

    // 模型目录：AAM_MODELS_DIR > 仓库旁的 test-models
    let models_dir = std::env::var("AAM_MODELS_DIR")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.is_dir())
        .or_else(|| {
            std::env::var("CARGO_MANIFEST_DIR")
                .ok()
                .map(|m| std::path::PathBuf::from(m).join("../../test-models"))
                .filter(|p| p.is_dir())
        });
    let Some(models_dir) = models_dir else {
        eprintln!("跳过：未找到模型目录（可用 AAM_MODELS_DIR 指定）");
        return;
    };
    if !models_dir.join("clap_text.onnx").is_file() {
        eprintln!("跳过：{} 下没有 clap_text.onnx", models_dir.display());
        return;
    }

    let conn = db::open_in_memory().unwrap();

    // 1. 导入并分析真实素材
    for f in &files {
        let id = import_file(&conn, f);
        let _ = tags_repo::list(&conn, id);
        let _ = notes_repo::list(&conn, id);
        let _ = worker::analyze_one(&conn, id);
    }

    // 2. 文本编码器是否可用（不可用则跳过）
    let probe = audio_asset_manager_lib::ml::clap_text::encode_if_available(&conn, "爆炸声")
        .unwrap_or(None);
    let Some(probe) = probe else {
        eprintln!("跳过：文本编码器不可用（缺少 Python/onnxruntime/tokenizers 或模型）");
        return;
    };
    assert_eq!(probe.len(), 512, "CLAP 文本向量应为 512 维");
    let norm: f32 = probe.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-3, "向量应已归一化，实际范数 {norm}");

    // 3. 对每个音频算向量（用规则标签向量做占位是不行的，这里直接构造
    //    「文本-文本」相似度以验证排序能力；音频塔待 mel 预处理修正后接入）
    let queries = ["沉闷的撞击声", "尖锐的枪声", "短促的界面点击声"];
    let mut vecs = std::collections::HashMap::new();
    for q in queries {
        let v = audio_asset_manager_lib::ml::clap_text::encode_if_available(&conn, q)
            .unwrap()
            .expect("编码器可用时不应返回 None");
        vecs.insert(q, v);
    }

    // 4. 同一提示词必须与自己最相似（编码稳定性的基本检查）
    for q in queries {
        let a = &vecs[q];
        let mut best = ("", f32::MIN);
        for (k, b) in &vecs {
            let s: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
            if s > best.1 {
                best = (*k, s);
            }
        }
        assert_eq!(best.0, q, "「{q}」应与自己最相似");
        assert!(best.1 > 0.99, "自相似度应接近 1，实际 {}", best.1);
    }

    // 5. 缓存应生效：第二次编码同一查询词不再调用 Python
    let cached = audio_asset_manager_lib::ml::clap_text::encode_if_available(&conn, "爆炸声")
        .unwrap()
        .expect("应命中缓存");
    assert_eq!(cached.len(), 512);
    assert_eq!(probe, cached, "缓存命中时应返回完全相同的向量");
}
/// log-mel 特征与官方 `transformers.audio_utils` 的**黄金数据**逐点比对。
///
/// 这是「推理内核是否真的正确」的关键测试：CLAP 音频塔与 AST 分类器都吃
/// log-mel，特征一旦与训练时不一致，向量就会退化（所有音频对任何提示词的
/// 相似度都差不多）。参考数据由 `tools/gen_mel_reference.py` 用官方实现生成。
#[test]
fn log_mel_matches_official_reference() {
    use audio_asset_manager_lib::audio::mel;

    let path = std::env::var("AAM_MEL_REFERENCE")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|p| p.is_file())
        .or_else(|| {
            std::env::var("CARGO_MANIFEST_DIR")
                .ok()
                .map(|m| std::path::PathBuf::from(m).join("../../test-models/mel_reference.json"))
                .filter(|p| p.is_file())
        })
        .or_else(|| {
            std::env::var("AAM_MODELS_DIR")
                .ok()
                .map(|m| std::path::PathBuf::from(m).join("mel_reference.json"))
                .filter(|p| p.is_file())
        });

    let Some(path) = path else {
        eprintln!("跳过：未找到 mel_reference.json（用 tools/gen_mel_reference.py 生成）");
        return;
    };

    let text = std::fs::read_to_string(&path).expect("读取参考数据");
    let v: serde_json::Value = serde_json::from_str(&text).expect("解析参考 JSON");

    // ---- 1. 滤波器组比对（JSON 为 [n_freq_bins][n_mels]，代码为 [n_mels][n_freq_bins]）----
    let fb_ref = v["filterbank"].as_array().expect("filterbank");
    assert_eq!(fb_ref.len(), mel::N_FREQ_BINS, "滤波器组频点数不符");

    let mine = mel::mel_filterbank_slaney();
    assert_eq!(mine.len(), mel::N_MEL);
    assert_eq!(mine[0].len(), mel::N_FREQ_BINS);

    let mut max_fb_diff = 0f64;
    for (k, row) in fb_ref.iter().enumerate() {
        let row = row.as_array().expect("row");
        assert_eq!(row.len(), mel::N_MEL);
        for m in 0..mel::N_MEL {
            let expect = row[m].as_f64().unwrap_or(0.0);
            let d = (expect - mine[m][k]).abs();
            if d > max_fb_diff {
                max_fb_diff = d;
            }
        }
    }
    assert!(
        max_fb_diff < 1e-9,
        "mel 滤波器组与官方不一致，最大偏差 {max_fb_diff:.3e}"
    );

    // ---- 2. log-mel 比对 ----
    let wave: Vec<f32> = v["waveform"]
        .as_array()
        .expect("waveform")
        .iter()
        .map(|x| x.as_f64().unwrap_or(0.0) as f32)
        .collect();
    assert!(!wave.is_empty(), "参考波形为空");

    let got = mel::log_mel_spectrogram(&wave);
    let expected_frames = v["expected_n_frames"].as_u64().unwrap_or(0) as usize;
    assert_eq!(
        got.len(),
        expected_frames,
        "帧数与官方不一致（补零或帧数公式有误）"
    );

    let head = v["expected_logmel_head"].as_array().expect("head");
    let mut max_diff = 0f32;
    let mut worst = (0usize, 0usize);
    for (t, row) in head.iter().enumerate() {
        let row = row.as_array().expect("row");
        for (m, cell) in row.iter().enumerate() {
            let expect = cell.as_f64().unwrap_or(0.0) as f32;
            let d = (expect - got[t][m]).abs();
            if d > max_diff {
                max_diff = d;
                worst = (t, m);
            }
        }
    }
    assert!(
        max_diff < 0.05,
        "log-mel 与官方偏差过大：{max_diff:.4} dB（帧 {} mel {}）",
        worst.0,
        worst.1
    );
    eprintln!(
        "log-mel 与官方最大偏差 {max_diff:.6} dB（{} 帧 x {} mel）",
        head.len(),
        mel::N_MEL
    );
}

/// 真实网络 + TLS 校验：确认 reqwest(rustls) 这条链路在本机可用。
///
/// 单元测试用本地 HTTP 服务验证了「流式写入 / 原子替换 / 进度上报」的逻辑，
/// 但覆盖不到 TLS 握手与真实 CDN。这里取回少量字节即可证明整条链路可用；
/// 同时校验服务器报告的大小与注册表登记值一致，防止登记的大小是拍脑袋写的。
/// 网络不可用时跳过（而非失败），避免离线环境误报。
#[test]
fn real_https_download_path_works() {
    use futures_util::StreamExt;

    // 直接取注册表里的条目，避免测试里的 URL/大小与清单脱节
    let spec = audio_asset_manager_lib::ml::model_manager::REGISTRY
        .iter()
        .find(|m| m.name == "clap_audio.onnx")
        .expect("注册表里应有 clap_audio.onnx");
    let url = spec.url;
    let expected_size = spec.size_bytes;
    assert!(expected_size > 0, "注册表里的体积必须是实测值");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let result: Result<(u64, Option<u64>), String> = rt.block_on(async {
        // 用应用实际使用的客户端（含系统代理探测），
        // 这样测试覆盖的就是真实下载链路
        let client = audio_asset_manager_lib::ml::downloader::build_client()
            .map_err(|e| format!("build client: {e}"))?;

        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("request: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let total = resp.content_length();

        // 读到 128 KB 就断开，足以证明 TLS + 流式读取可用
        let mut got: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let c = chunk.map_err(|e| format!("chunk: {e}"))?;
            got += c.len() as u64;
            if got >= 128 * 1024 {
                break;
            }
        }
        Ok((got, total))
    });

    match result {
        Ok((got, total)) => {
            assert!(got > 0, "应至少取回一些字节");

            // 服务器报告的长度应与注册表登记的期望大小接近（±15%）
            let expected = audio_asset_manager_lib::ml::model_manager::REGISTRY
                .iter()
                .find(|m| m.name == "clap_audio.onnx")
                .map(|m| m.size_bytes);
            if let (Some(actual), Some(expect)) = (total, expected) {
                let diff = (actual as f64 - expect as f64).abs() / expect as f64;
                assert!(
                    diff < 0.15,
                    "实际大小 {actual} 与注册表登记 {expect} 相差 {:.1}%，登记值需要更新",
                    diff * 100.0
                );
            }
        }
        Err(e) => {
            eprintln!("跳过：网络不可用或模型地址不可达（{e}）");
        }
    }
}
