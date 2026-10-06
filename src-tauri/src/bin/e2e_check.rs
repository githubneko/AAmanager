/// 端到端验证（开发期）：把素材导入真实库、跑完整分析流程、再做语义检索。
///
/// 这是「功能是否真的可用」的检验 —— 单测覆盖不到 ONNX 推理与检索排序。
///
/// 用法：cargo run --bin e2e_check -- <素材目录> <模型目录>
use audio_asset_manager_lib::db;
use audio_asset_manager_lib::indexer::worker;
use audio_asset_manager_lib::ml::{clap_audio, clap_text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: e2e_check <素材目录> <模型目录>");
        std::process::exit(2);
    }
    let sfx = std::path::Path::new(&args[1]);
    let models = std::path::Path::new(&args[2]);

    // 用临时库，避免污染用户真实资料库
    let mut db_path = std::env::temp_dir();
    db_path.push(format!("aam_e2e_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    // 模型目录通过环境变量覆盖：effective_models_dir 会优先读它
    std::env::set_var("AAM_MODELS_DIR", models);

    let conn = db::open(&db_path)?;
    println!("临时库: {}", db_path.display());
    println!("模型目录: {}", models.display());

    // ---- 1. 导入 + 分析 ----
    let mut ids = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(sfx)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && matches!(
                    p.extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .as_str(),
                    "wav" | "mp3" | "ogg" | "flac" | "m4a"
                )
        })
        .collect();
    entries.sort();
    for p in &entries {
        let decoded = audio_asset_manager_lib::audio::decoder::decode(p, 600.0)?;
        let new = db::audio_repo::NewAudio {
            path: p.to_string_lossy().to_string(),
            filename: p.file_name().unwrap().to_string_lossy().to_string(),
            folder: p
                .parent()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_default(),
            duration: Some(decoded.duration),
            sample_rate: Some(decoded.sample_rate as i64),
            channels: Some(decoded.channels as i64),
            format: decoded.format.clone(),
            bitrate: decoded.bitrate.map(|b| b as i64),
            file_size: std::fs::metadata(p).ok().map(|m| m.len() as i64),
            file_hash: None,
        };
        let id = db::audio_repo::insert(&conn, &new)?;
        ids.push((id, p.file_stem().unwrap().to_string_lossy().to_string()));
    }
    println!("已导入 {} 个素材", ids.len());

    for (id, name) in &ids {
        match worker::analyze_one(&conn, *id) {
            Ok(_) => {}
            Err(e) => println!("  分析失败 {name}: {e}"),
        }
    }

    // ---- 2. 向量统计 ----
    let n_vec = db::vector_repo::count_embeddings(&conn)?;
    println!();
    println!("=== 向量索引 ===");
    println!("  音频向量数: {n_vec} / {}", ids.len());
    if n_vec == 0 {
        println!("  ❌ 没有生成任何音频向量，语义检索无法工作");
        return Ok(());
    }

    // ---- 3. 跨模态检索 ----
    let queries = [
        "a pop song with drums and vocals",
        "a bell chiming",
        "an alarm clock beeping",
        "a telephone ringing",
        "continuous ambient noise",
    ];

    println!();
    println!("=== 语义检索结果（Top-3）===");
    let candidates = db::audio_repo::all_with_embeddings(&conn)?;
    println!("  候选音频向量 {} 条", candidates.len());

    let name_of = |id: i64| -> String {
        ids.iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| format!("id={id}"))
    };

    for q in queries {
        let vec = match clap_text::encode_if_available(&conn, q)? {
            Some(v) => v,
            None => {
                println!("  「{q}」编码失败（模型或分词器不可用）");
                continue;
            }
        };
        let hits = audio_asset_manager_lib::ml::similarity::top_k(&vec, &candidates, 3, 0.0);
        let rendered: Vec<String> = hits
            .iter()
            .map(|(id, score)| format!("{}({:+.3})", name_of(*id), score))
            .collect();
        println!("  {q:38} -> {}", rendered.join("  "));
    }

    // ---- 4. 模型自检 ----
    println!();
    println!("=== 自检 ===");
    println!("  语义检索就绪: {}", clap_text::is_available(&conn));
    println!(
        "  音频模型存在: {}",
        models.join(clap_audio::MODEL_NAME).is_file()
    );

    let _ = std::fs::remove_file(&db_path);
    Ok(())
}
