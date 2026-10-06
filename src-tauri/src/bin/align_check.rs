//! 端到端跨模态对齐验证（开发期）：用 Rust 侧两个编码器测检索是否正确。
//!
//! 这是判断「语义搜索能不能开」的最终依据：文本塔与音频塔各自正确还不够，
//! 必须**互相可比**。
//!
//! 用法：cargo run --bin align_check -- <模型目录> <素材目录>

use audio_asset_manager_lib::audio::decoder;
use audio_asset_manager_lib::ml::{clap_audio, clap_text};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: align_check <模型目录> <素材目录>");
        std::process::exit(2);
    }
    let models = Path::new(&args[1]);
    let sfx = Path::new(&args[2]);

    let tok = clap_text::load_tokenizer(&models.join(clap_text::TOKENIZER_NAME))?;
    let text_model = models.join(clap_text::MODEL_NAME);
    let audio_model = models.join(clap_audio::MODEL_NAME);

    // ---- 音频向量 ----
    let mut audio: Vec<(String, Vec<f32>)> = Vec::new();
    for entry in std::fs::read_dir(sfx)? {
        let p = entry?.path();
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !matches!(ext.as_str(), "wav" | "mp3" | "ogg" | "flac" | "m4a") {
            continue;
        }
        if !p.is_file() {
            continue;
        }
        let decoded = decoder::decode(&p, 600.0)?;
        let feats =
            audio_asset_manager_lib::audio::mel::clap_input(&decoded.samples, decoded.sample_rate)?;
        let v = clap_audio::embed_features(
            audio_asset_manager_lib::ml::inference::global(),
            &audio_model,
            &feats,
        )?;
        audio.push((p.file_stem().unwrap().to_string_lossy().to_string(), v));
    }
    if audio.is_empty() {
        eprintln!("素材目录里没有音频");
        std::process::exit(2);
    }

    // ---- 文本向量 ----
    let prompts = [
        "a ringing bell",
        "an alarm siren",
        "a soft chime",
        "explosion",
        "a sharp gunshot",
        "continuous ambient noise",
        "a short ui click",
    ];
    let mut text: Vec<(&str, Vec<f32>)> = Vec::new();
    for p in prompts {
        let ids = clap_text::encode_ids(&tok, p)?;
        let v = clap_text::embed_ids(&text_model, &ids)?;
        text.push((p, v));
    }

    let dot = |a: &[f32], b: &[f32]| -> f32 { a.iter().zip(b).map(|(x, y)| x * y).sum() };

    println!("=== 音频 x 文本 余弦（* = 该行最大值）===");
    print!("{:>26}", "");
    for (n, _) in &audio {
        print!("{:>13}", &n[..n.len().min(11)]);
    }
    println!();
    for (p, tv) in &text {
        let mut row: Vec<f32> = audio.iter().map(|(_, av)| dot(tv, av)).collect();
        let best = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        print!("{p:>26}");
        for (i, s) in row.iter_mut().enumerate() {
            if i == best {
                print!("{s:>12.3}*");
            } else {
                print!("{s:>13.3}");
            }
        }
        println!();
    }

    println!();
    println!("=== 每个音频最匹配的文本 ===");
    let mut flat = 0.0f32;
    let mut count = 0usize;
    for (i, (n, av)) in audio.iter().enumerate() {
        let sims: Vec<f32> = text.iter().map(|(_, tv)| dot(tv, av)).collect();
        let best = sims
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(j, _)| j)
            .unwrap_or(0);
        let sorted = {
            let mut s = sims.clone();
            s.sort_by(|a, b| b.partial_cmp(a).unwrap());
            s
        };
        let spread = sorted[0] - sorted[sorted.len() - 1];
        flat += spread;
        count += 1;
        println!(
            "  {n:18} -> {:26} ({:+.3})  极差 {:.3}",
            text[best].0, sims[best], spread
        );
        let _ = i;
    }
    println!();
    println!(
        "平均极差 {:.3}（越大说明文本能区分不同音频；接近 0 说明文本塔不敏感）",
        flat / count.max(1) as f32
    );
    Ok(())
}
