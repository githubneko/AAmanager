//! 开发期工具：把 Rust 侧的 log-mel 与模型输入导出为原始 f32，
//! 供 Python 侧与官方 processor 输出逐点比对。
//!
//! 用法：cargo run --bin mel_dump -- <音频文件> <输出文件>

use audio_asset_manager_lib::audio::{ast_features, decoder, mel};
use std::io::Write;

fn write_f32(path: &std::path::Path, data: &[f32]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    for v in data {
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    // 第三个参数可选：填 "ast" 时导出 AST 特征，否则导出 CLAP 特征
    if args.len() < 3 {
        eprintln!("用法: mel_dump <音频文件> <输出 .f32 文件> [ast]");
        std::process::exit(2);
    }
    let src = std::path::Path::new(&args[1]);
    let dst = std::path::Path::new(&args[2]);
    let want_ast = args.get(3).map(|s| s == "ast").unwrap_or(false);

    let decoded = decoder::decode(src, 600.0)?;

    if want_ast {
        let x = ast_features::ast_input(&decoded.samples, decoded.sample_rate)?;
        write_f32(dst, &x)?;
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let var = x.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / x.len() as f32;
        println!(
            "{}: AST 特征 {} 个（{} x {}） min={:.3} max={:.3} mean={:.3} std={:.3}",
            src.file_name().unwrap().to_string_lossy(),
            x.len(),
            ast_features::TARGET_FRAMES,
            ast_features::N_MEL,
            x.iter().cloned().fold(f32::INFINITY, f32::min),
            x.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
            mean,
            var.sqrt()
        );
        return Ok(());
    }

    let x = mel::clap_input(&decoded.samples, decoded.sample_rate)?;
    write_f32(dst, &x)?;
    println!(
        "{}: 输入 {} 样本 @ {} Hz -> CLAP 特征 {} 个 f32（{} x {}）",
        src.file_name().unwrap().to_string_lossy(),
        decoded.samples.len(),
        decoded.sample_rate,
        x.len(),
        mel::TARGET_FREQ,
        mel::TARGET_FRAMES
    );
    Ok(())
}
