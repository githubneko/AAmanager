//! 开发期工具：用 Rust 侧完整链路算**文本**向量并导出，供与 PyTorch 比对。
//!
//! 用法：cargo run --bin text_dump -- "<查询文本>" <输出 .f32> <模型目录>

use audio_asset_manager_lib::ml::clap_text;
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: text_dump <查询文本> <输出 .f32> <模型目录>");
        std::process::exit(2);
    }
    let text = &args[1];
    let dst = Path::new(&args[2]);
    let models = Path::new(&args[3]);

    let tok = clap_text::load_tokenizer(&models.join(clap_text::TOKENIZER_NAME))?;
    let ids = clap_text::encode_ids(&tok, text)?;
    let vec = clap_text::embed_ids(&models.join(clap_text::MODEL_NAME), &ids)?;

    let mut f = std::fs::File::create(dst)?;
    for v in &vec {
        f.write_all(&v.to_le_bytes())?;
    }

    println!(
        "「{}」-> {} 维，前 8 个 id: {:?}",
        text,
        vec.len(),
        &ids[..8]
    );
    println!(
        "  前 5 个分量: {:?}",
        vec[..5]
            .iter()
            .map(|v| (v * 10000.0).round() / 10000.0)
            .collect::<Vec<_>>()
    );
    Ok(())
}
