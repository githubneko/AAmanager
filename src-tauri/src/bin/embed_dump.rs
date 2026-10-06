//! 开发期工具：用 Rust 侧完整链路（解码 → log-mel → ONNX）算音频向量并导出，
//! 供与官方 PyTorch 输出逐点比对。
//!
//! 用法：cargo run --bin embed_dump -- <音频文件> <输出 .f32 文件> <模型目录>

use audio_asset_manager_lib::audio::decoder;
use audio_asset_manager_lib::ml::{clap_audio, inference};
use std::io::Write;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("用法: embed_dump <音频文件> <输出 .f32> <模型目录>");
        std::process::exit(2);
    }
    let src = Path::new(&args[1]);
    let dst = Path::new(&args[2]);
    let models = Path::new(&args[3]);

    let decoded = decoder::decode(src, 600.0)?;
    let features =
        audio_asset_manager_lib::audio::mel::clap_input(&decoded.samples, decoded.sample_rate)?;
    let vec = clap_audio::embed_features(
        inference::global(),
        &models.join(clap_audio::MODEL_NAME),
        &features,
    )?;

    let mut f = std::fs::File::create(dst)?;
    for v in &vec {
        f.write_all(&v.to_le_bytes())?;
    }

    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    println!(
        "{} -> {} 维向量（L2={:.6}），前 5 个: {:?}",
        src.file_name().unwrap().to_string_lossy(),
        vec.len(),
        norm,
        vec[..5]
            .iter()
            .map(|v| (v * 10000.0).round() / 10000.0)
            .collect::<Vec<_>>()
    );
    Ok(())
}
