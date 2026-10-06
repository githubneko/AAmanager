//! 开发期工具：用 AST 对素材做事件分类，检查是否合理。
//!
//! 用法：cargo run --bin ast_dump -- <模型目录> <素材目录>

use audio_asset_manager_lib::audio::decoder;
use audio_asset_manager_lib::ml::yamnet;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: ast_dump <模型目录> <素材目录>");
        std::process::exit(2);
    }
    let models = std::path::Path::new(&args[1]);
    let sfx = std::path::Path::new(&args[2]);

    let names = yamnet::load_class_names(&models.join(yamnet::CONFIG_NAME))?;
    println!("类别表载入 {} 类", names.len());

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
        let decoded = decoder::decode(p, 600.0)?;
        let t0 = std::time::Instant::now();
        let scores = yamnet::infer_with_path(
            &models.join(yamnet::MODEL_NAME),
            &decoded.samples,
            decoded.sample_rate,
        )?;
        let elapsed = t0.elapsed();

        // 模型输出是 logits，转成概率才有可比性
        let probs = yamnet::logits_to_probabilities(&scores);
        let mut idx: Vec<usize> = (0..probs.len()).collect();
        idx.sort_by(|a, b| probs[*b].partial_cmp(&probs[*a]).unwrap());

        println!();
        println!(
            "{} （{:.1} 秒音频，{:.0} ms）",
            p.file_name().unwrap().to_string_lossy(),
            decoded.duration,
            elapsed.as_secs_f32() * 1000.0
        );
        for &i in idx.iter().take(5) {
            println!(
                "    {:<44} {:>6.3}  -> {}",
                names[i],
                probs[i],
                yamnet::category_label(&names[i])
            );
        }
    }
    Ok(())
}
