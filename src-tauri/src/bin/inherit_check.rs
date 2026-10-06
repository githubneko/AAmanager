//! 端到端验证（开发期）：相似度标签继承。
//!
//! 用**真实模型**跑完整链路：造两个几乎相同的音频 → 导入并分析（生成向量）
//! → 给其中一个打用户标签 → 触发继承 → 检查另一个是否真的拿到了标签。
//!
//! 用法：cargo run --bin inherit_check -- <模型目录>

use audio_asset_manager_lib::db;
use audio_asset_manager_lib::ml::inherit;
use audio_asset_manager_lib::models::TagSource;
use rusqlite::Connection;

/// 生成一段测试音频：`freq` 决定内容，`jitter` 用于制造「几乎相同」的变体。
fn write_wav(path: &std::path::Path, freq: f32, jitter: f32, secs: f32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (48_000.0 * secs) as u32;
    for i in 0..n {
        let t = i as f32 / 48_000.0;
        // 主音 + 一个固定泛音，jitter 只做极小的频率偏移
        let v = (t * freq * std::f32::consts::TAU).sin() * 0.6
            + (t * freq * 2.0 * (1.0 + jitter) * std::f32::consts::TAU).sin() * 0.25;
        w.write_sample((v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

fn mk_audio(conn: &Connection, path: &std::path::Path) -> i64 {
    let decoded = audio_asset_manager_lib::audio::decoder::decode(path, 600.0).unwrap();
    db::audio_repo::insert(
        conn,
        &db::audio_repo::NewAudio {
            path: path.to_string_lossy().to_string(),
            filename: path.file_name().unwrap().to_string_lossy().to_string(),
            folder: path.parent().unwrap().to_string_lossy().to_string(),
            duration: Some(decoded.duration),
            sample_rate: Some(decoded.sample_rate as i64),
            channels: Some(decoded.channels as i64),
            format: decoded.format.clone(),
            bitrate: None,
            file_size: std::fs::metadata(path).ok().map(|m| m.len() as i64),
            file_hash: None,
        },
    )
    .unwrap()
}

fn analyze(conn: &Connection, id: i64) {
    audio_asset_manager_lib::indexer::worker::analyze_one(conn, id).unwrap();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: inherit_check <模型目录>");
        std::process::exit(2);
    }
    std::env::set_var("AAM_MODELS_DIR", &args[1]);

    let dir = std::env::temp_dir().join(format!("aam_inherit_{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let mut db_path = dir.clone();
    db_path.push("lib.db");
    let _ = std::fs::remove_file(&db_path);
    let conn = db::open(&db_path)?;

    // 同一个「音效」的三个变体 + 一个无关音频
    let a = dir.join("laser_a.wav");
    let b = dir.join("laser_b.wav");
    let c = dir.join("laser_c.wav");
    let other = dir.join("ambient.wav");
    write_wav(&a, 1200.0, 0.0, 1.5);
    write_wav(&b, 1200.0, 0.0005, 1.5);
    write_wav(&c, 1200.0, 0.0008, 1.5);
    write_wav(&other, 90.0, 0.0, 1.5); // 完全不同的低频音

    let (ia, ib, ic, io) = (
        mk_audio(&conn, &a),
        mk_audio(&conn, &b),
        mk_audio(&conn, &c),
        mk_audio(&conn, &other),
    );

    println!("=== 分析（生成向量 + 规则标签）===");
    for (id, name) in [
        (ia, "laser_a"),
        (ib, "laser_b"),
        (ic, "laser_c"),
        (io, "ambient"),
    ] {
        analyze(&conn, id);
        let n = db::vector_repo::count_embeddings(&conn)?;
        println!("  {name}: 库内向量累计 {n}");
    }

    // 相邻变体的相似度
    let va = db::vector_repo::get_embedding(&conn, ia)?.unwrap();
    let vb = db::vector_repo::get_embedding(&conn, ib)?.unwrap();
    let vc = db::vector_repo::get_embedding(&conn, ic)?.unwrap();
    let vo = db::vector_repo::get_embedding(&conn, io)?.unwrap();
    let sim = audio_asset_manager_lib::ml::similarity::cosine_similarity;
    println!();
    println!("=== 向量相似度 ===");
    println!("  laser_a vs laser_b : {:.4}", sim(&va, &vb));
    println!("  laser_a vs laser_c : {:.4}", sim(&va, &vc));
    println!("  laser_a vs ambient : {:.4}", sim(&va, &vo));

    // 给 laser_a 打一个用户标签（模拟用户手动命名）
    db::tags_repo::add_tag(&conn, ia, "主角激光枪", TagSource::User, 1.0)?;
    println!();
    println!("已给 laser_a 打上用户标签「主角激光枪」");

    // 触发全库继承
    let (processed, inherited, pruned) = inherit::inherit_all(&conn)?;
    println!("inherit_all: 处理 {processed} 个，继承 {inherited} 个，清理 {pruned} 个");

    println!();
    println!("=== 各音频最终标签 ===");
    let mut ok = true;
    for (id, name) in [
        (ia, "laser_a"),
        (ib, "laser_b"),
        (ic, "laser_c"),
        (io, "ambient"),
    ] {
        let tags = db::tags_repo::list(&conn, id)?;
        let inherited_tags: Vec<&str> = tags
            .iter()
            .filter(|t| t.source == TagSource::Inherited)
            .map(|t| t.tag.as_str())
            .collect();
        let conf = tags
            .iter()
            .find(|t| t.source == TagSource::Inherited)
            .map(|t| t.confidence);
        println!(
            "  {name:10} 共 {} 个标签，继承: {:?} {}",
            tags.len(),
            inherited_tags,
            conf.map(|c| format!("(置信度 {c:.3})")).unwrap_or_default()
        );
        if name != "laser_a" && name != "ambient" && inherited_tags != vec!["主角激光枪"] {
            ok = false;
        }
    }

    // ambient 不应继承（相似度低）
    let ambient_inherited: Vec<String> = db::tags_repo::list(&conn, io)?
        .into_iter()
        .filter(|t| t.source == TagSource::Inherited)
        .map(|t| t.tag)
        .collect();

    println!();
    println!("================ VERDICT ================");
    if ok && ambient_inherited.is_empty() {
        println!("PASS 相似变体继承了用户标签，无关音频未受影响");
    } else if !ambient_inherited.is_empty() {
        println!("FAIL 无关音频也被继承了: {ambient_inherited:?}");
    } else {
        println!("FAIL 部分相似变体未继承到标签");
    }

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
