//! 开发期工具：对真实资料库跑一批查询，检查切词与词汇表展开是否真的生效。
//!
//! 用法：cargo run --bin search_check -- <库路径> [词汇表]
//!
//! 只读：不会修改任何数据。

use audio_asset_manager_lib::db::audio_repo;
use audio_asset_manager_lib::models::{KeywordMode, SearchQuery};
use audio_asset_manager_lib::tag_vocab::{Resolver, TagVocab};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("用法: search_check <库路径> [game|video|music|custom]");
        std::process::exit(2);
    }
    let vocab = match args.get(2).map(|s| s.as_str()) {
        Some("video") => TagVocab::Video,
        Some("music") => TagVocab::Music,
        Some("custom") => TagVocab::Custom,
        _ => TagVocab::Game,
    };
    let resolver = Resolver::new(vocab, std::collections::HashMap::new());
    let conn = audio_asset_manager_lib::db::open(std::path::Path::new(&args[1]))?;

    println!("词汇表: {vocab:?}");
    println!();

    // 展示几条对照，确认「显示名 != 存储名」这件事
    println!("=== 词表对照（规范名 -> 当前显示名）===");
    for canonical in ["沉闷", "尖锐", "宽带噪声", "低频"] {
        println!("  {canonical:8} -> {}", resolver.label(canonical));
    }
    println!();

    let queries = [
        ("沉闷的爆炸", KeywordMode::And),
        ("沉闷 爆炸", KeywordMode::And),
        ("沉闷", KeywordMode::And),
        ("爆炸", KeywordMode::And),
        ("爆炸/轰鸣", KeywordMode::And),
        ("轰鸣", KeywordMode::And),
        ("激光", KeywordMode::And),
        ("枪击", KeywordMode::And),
        ("沉闷的爆炸 激光", KeywordMode::And),
        ("gunshot explosion", KeywordMode::And),
    ];

    println!("=== 检索结果 ===");
    for (text, mode) in queries {
        // 必须复刻命令层的做法：把词汇展开结果放进 extra_terms，
        // 否则测的就不是用户实际走的那条路径。
        let extra = resolver.expand_query(text);
        let q = SearchQuery {
            text: text.to_string(),
            extra_terms: extra.clone(),
            mode,
            limit: 20,
            ..Default::default()
        };
        let (items, total) = audio_repo::query(&conn, &q)?;

        let names: Vec<String> = items
            .iter()
            .take(5)
            .map(|i| {
                let tags: Vec<&str> = i.tags.iter().map(|t| t.tag.as_str()).collect();
                format!("{}({})", i.audio.filename, tags.join("+"))
            })
            .collect();

        let mark = if total == 0 { "✗" } else { "✓" };
        println!(
            "  {mark} {text:18} 切词={:?}",
            audio_asset_manager_lib::text::tokenize(text)
        );
        if !extra.is_empty() {
            println!("      展开 -> {extra:?}");
        }
        println!("      命中 {total} 条: {}", names.join("  "));
    }
    Ok(())
}
