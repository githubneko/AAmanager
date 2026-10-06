//! 音频事件分类（AST / AudioSet 527 类）→ 自动标签。
//!
//! ## 为什么是 AST 而不是 YAMNet
//!
//! 策划书 3.3 指定 YAMNet，但其 ONNX 导出在可访问的仓库中都不存在（实测 404）。
//! AST（Audio Spectrogram Transformer）同样是 AudioSet 多标签分类器、
//! 类别集完全一致（527 类），且精度更高。为与策划书保持一致的调用面，
//! 磁盘文件名仍沿用 `yamnet.onnx`。
//!
//! ## 输入输出
//!
//! * 输入 `input_values`：`[1, 1024, 128]`，16 kHz 128 维 log-mel
//!   （见 [`crate::audio::ast_features`]）；
//! * 输出 `logits`：`[1, 527]`，对应 AudioSet 的 527 个类别。
//!
//! ## 类别名 → 中文标签
//!
//! `yamnet_config.json` 里的 `id2label` 是英文（`Gunshot, gunfire` 等）。
//! 直接展示英文对中文用户不友好，因此内置一张常用类别的中英映射表
//! （见 [`category_label`]）；表里没有的类别退回英文原名，
//! 保证任何类别都能显示，而不是被丢掉。

use crate::audio::ast_features;
use crate::audio::decoder::DecodedAudio;
use crate::error::{AppError, Result};
use crate::ml::model_manager::ModelManager;
use crate::models::{Tag, TagSource};
use rusqlite::Connection;

/// 模型逻辑名（与 [`crate::ml::model_manager::REGISTRY`] 对应）。
pub const MODEL_NAME: &str = "yamnet.onnx";

/// 类别标签配置文件名。
pub const CONFIG_NAME: &str = "yamnet_config.json";

/// 低于该置信度的标签视为「待确认」。
pub const LOW_CONFIDENCE: f32 = 0.30;

/// 只保留 Top-N 标签。
pub const TOP_N: usize = 5;

/// 指定模型路径执行推理，返回 527 维 logits。
pub fn infer_with_path(
    model_path: &std::path::Path,
    samples: &[f32],
    sample_rate: u32,
) -> Result<Vec<f32>> {
    let input = ast_features::ast_input(samples, sample_rate)?;
    let shape = [
        1i64,
        ast_features::TARGET_FRAMES as i64,
        ast_features::N_MEL as i64,
    ];
    let tensor = ort::value::Tensor::from_array((shape, input))
        .map_err(|e| AppError::Audio(format!("构造 AST 输入张量失败: {e}")))?;

    crate::ml::inference::global().with_session(model_path, |session| {
        let outputs = session
            .run(ort::inputs!["input_values" => tensor])
            .map_err(|e| AppError::Audio(format!("AST 推理失败: {e}")))?;
        let (shape, data) = outputs
            .get("logits")
            .ok_or_else(|| AppError::Audio("AST 输出缺少 logits".into()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Audio(format!("解析 AST 输出失败: {e}")))?;
        let v: Vec<f32> = data.to_vec();
        if v.len() != ast_features::NUM_CLASSES {
            return Err(AppError::Audio(format!(
                "AST 类别数异常: 期望 {}，实际 {}（shape {shape:?}）",
                ast_features::NUM_CLASSES,
                v.len()
            )));
        }
        Ok(v)
    })
}

/// 读取 `id2label` 类别名表。
pub fn load_class_names(path: &std::path::Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::NotFound(format!("读取类别配置失败 {}: {e}", path.display())))?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Audio(format!("解析类别配置失败: {e}")))?;
    let map = v
        .get("id2label")
        .and_then(|m| m.as_object())
        .ok_or_else(|| AppError::Audio("类别配置里没有 id2label".into()))?;

    // 按下标顺序取出，缺号用占位符补，保证下标与模型输出对齐
    let mut names = vec![String::new(); ast_features::NUM_CLASSES];
    for (k, val) in map {
        if let Ok(i) = k.parse::<usize>() {
            if i < names.len() {
                names[i] = val.as_str().unwrap_or_default().to_string();
            }
        }
    }
    Ok(names)
}

/// AudioSet 类别名 → 展示用中文标签。
///
/// 表里没有的类别原样返回英文名 —— 宁可显示英文，也不要丢掉一个真实命中的类别。
pub fn category_label(english: &str) -> String {
    let key = english.to_ascii_lowercase();
    // 用子串匹配，因为 AudioSet 的类名常带补充说明（如 "Gunshot, gunfire"）
    const TABLE: &[(&str, &str)] = &[
        ("gunshot", "枪声"),
        ("machine gun", "机枪声"),
        ("fusillade", "连发枪声"),
        ("artillery", "炮击声"),
        ("explosion", "爆炸声"),
        ("boom", "轰鸣"),
        ("burst", "爆裂声"),
        ("cap gun", "玩具枪声"),
        ("firecracker", "鞭炮声"),
        ("fireworks", "烟花声"),
        ("glass", "玻璃破碎"),
        ("shatter", "碎裂"),
        ("thump", "沉闷撞击"),
        ("thud", "闷响"),
        ("bang", "巨响"),
        ("knock", "敲击声"),
        ("tap", "轻敲"),
        ("click", "咔哒声"),
        ("clatter", "咔啦声"),
        ("slam", "摔门声"),
        ("crash", "撞击碎裂"),
        ("smash", "猛撞"),
        ("impact", "撞击声"),
        ("squeak", "吱吱声"),
        ("creak", "嘎吱声"),
        ("rustle", "沙沙声"),
        ("whoosh", "呼啸声"),
        ("swoosh", "嗖声"),
        ("whistle", "口哨/啸叫"),
        ("beep", "提示音"),
        ("buzzer", "蜂鸣器"),
        ("alarm clock", "闹钟"),
        ("alarm", "警报声"),
        ("siren", "警笛"),
        ("bell", "铃声"),
        ("chime", "钟声"),
        ("ding", "叮声"),
        ("gong", "锣声"),
        ("ringtone", "手机铃声"),
        ("telephone", "电话声"),
        ("dtmf", "拨号音"),
        ("dial", "拨号"),
        ("doorbell", "门铃"),
        ("bicycle bell", "车铃"),
        ("church bell", "教堂钟声"),
        ("music", "音乐"),
        ("singing", "歌声"),
        ("choir", "合唱"),
        ("instrument", "乐器声"),
        ("guitar", "吉他"),
        ("piano", "钢琴"),
        ("drum", "鼓声"),
        ("bass", "贝斯/低频"),
        ("violin", "小提琴"),
        ("flute", "长笛"),
        ("trumpet", "小号"),
        ("saxophone", "萨克斯"),
        ("organ", "风琴"),
        ("synthesizer", "合成器"),
        ("sampler", "采样器"),
        ("beatbox", "口技鼓点"),
        ("hip hop", "嘻哈"),
        ("rock", "摇滚"),
        ("pop", "流行"),
        ("jazz", "爵士"),
        ("classical", "古典"),
        ("electronic", "电子乐"),
        ("dance", "舞曲"),
        ("ambient", "氛围音"),
        ("speech", "人声说话"),
        ("male speech", "男声"),
        ("female speech", "女声"),
        ("conversation", "交谈"),
        ("narration", "旁白"),
        ("whisper", "低语"),
        ("laughter", "笑声"),
        ("giggle", "咯咯笑"),
        ("crying", "哭声"),
        ("scream", "尖叫声"),
        ("shout", "喊叫"),
        ("sigh", "叹息"),
        ("cough", "咳嗽"),
        ("sneeze", "喷嚏"),
        ("breathing", "呼吸声"),
        ("snoring", "鼾声"),
        (" Applause", "掌声"),
        ("applause", "掌声"),
        ("clapping", "拍手"),
        ("cheering", "欢呼"),
        ("footsteps", "脚步声"),
        ("walk", "走路声"),
        ("run", "跑步声"),
        ("water", "水声"),
        ("rain", "雨声"),
        ("thunder", "雷声"),
        ("wind", "风声"),
        ("storm", "暴风雨"),
        ("stream", "溪流声"),
        ("drip", "滴水声"),
        ("pour", "倒水声"),
        ("splash", "溅水声"),
        ("gurgle", "汩汩声"),
        ("fire", "火焰声"),
        ("crackle", "噼啪声"),
        ("engine", "引擎声"),
        ("car", "汽车声"),
        ("motorcycle", "摩托声"),
        ("truck", "卡车声"),
        ("bus", "公交车声"),
        ("train", "火车声"),
        ("aircraft", "飞机声"),
        ("helicopter", "直升机"),
        ("boat", "船声"),
        ("sirens", "警笛"),
        ("horn", "喇叭"),
        ("reverse beeper", "倒车提示音"),
        ("tire", "轮胎声"),
        ("skateboard", "滑板声"),
        ("vacuum", "吸尘器"),
        ("lawn mower", "割草机"),
        ("chainsaw", "电锯"),
        ("power tool", "电动工具"),
        ("drill", "电钻"),
        ("saw", "锯声"),
        ("hammer", "锤击"),
        ("printer", "打印机"),
        ("camera", "相机声"),
        ("shutter", "快门声"),
        ("keyboard", "键盘声"),
        ("typing", "打字声"),
        ("mouse click", "鼠标点击"),
        ("computer", "电脑声"),
        ("fan", "风扇声"),
        ("air conditioning", "空调声"),
        ("clock", "钟表声"),
        ("tick", "滴答声"),
        ("silence", "静音"),
        ("noise", "噪声"),
        ("white noise", "白噪声"),
        ("pink noise", "粉红噪声"),
        ("static", "静电噪声"),
        ("hum", "嗡鸣"),
        ("hiss", "嘶嘶声"),
        ("rumble", "隆隆声"),
        ("drone", "低鸣"),
        ("animal", "动物声"),
        ("dog", "狗叫"),
        ("cat", "猫叫"),
        ("bird", "鸟叫"),
        ("insect", "虫鸣"),
        ("frog", "蛙鸣"),
        ("crow", "乌鸦叫"),
        ("chicken", "鸡叫"),
        ("cow", "牛叫"),
        ("horse", "马嘶"),
        ("sheep", "羊叫"),
        ("pig", "猪叫"),
        ("lion", "狮吼"),
        ("whale", "鲸叫"),
        ("cricket", "蟋蟀"),
        ("bee", "蜜蜂"),
        ("mosquito", "蚊子"),
        ("door", "开门关门"),
        ("drawer", "抽屉声"),
        ("zipper", "拉链声"),
        ("keys", "钥匙声"),
        ("coin", "硬币声"),
        ("paper", "纸张声"),
        ("writing", "书写声"),
        ("tearing", "撕纸声"),
        ("crushing", "碾压声"),
        ("chopping", "切菜声"),
        ("frying", "煎炸声"),
        ("sizzle", "滋滋声"),
        ("microwave", "微波炉"),
        ("blender", "搅拌机"),
        ("food", "食物相关"),
        ("eating", "咀嚼声"),
        ("drinking", "喝水声"),
        ("sipping", "啜饮声"),
        ("burp", "打嗝"),
        ("chewing", "咀嚼"),
        ("heartbeat", "心跳声"),
        ("breath", "呼吸"),
        ("gasp", "喘气"),
        ("wheeze", "喘息"),
        ("sniff", "吸鼻声"),
        ("hiccup", "打嗝"),
        ("belch", "打嗝"),
        ("fart", "放屁声"),
        ("burping", "打嗝"),
        ("sneeze", "喷嚏"),
        ("applause", "掌声"),
        ("crowd", "人群声"),
        ("children", "儿童声"),
        ("baby", "婴儿声"),
        ("sports", "运动声"),
        ("basketball", "篮球声"),
        ("soccer", "足球声"),
        ("swimming", "游泳声"),
        ("bowling", "保龄球"),
        ("golf", "高尔夫"),
        ("tennis", "网球"),
        ("skate", "滑行声"),
        ("surf", "冲浪声"),
        ("fireworks", "烟花"),
        ("gun", "枪械声"),
        ("weapon", "武器声"),
        ("sword", "刀剑声"),
        ("whip", "鞭打声"),
        ("slap", "拍打声"),
        ("punch", "击打声"),
        ("kick", "踢击声"),
        ("clap", "拍手声"),
        ("snap", "折断声"),
        ("crack", "开裂声"),
        ("rip", "撕裂声"),
        ("tear", "撕裂"),
        ("squish", "挤压声"),
        ("splat", "啪嗒声"),
        ("squelch", "咕叽声"),
        ("boing", "弹簧声"),
        ("squeal", "尖鸣"),
        ("screech", "刺耳尖叫"),
        ("howl", "嚎叫"),
        ("growl", "低吼"),
        ("roar", "咆哮"),
        ("yell", "大喊"),
        ("chant", "吟唱"),
        ("mantra", "诵经"),
        ("humming", "哼唱"),
        ("whistling", "吹口哨"),
        ("yodeling", "约德尔唱法"),
        ("rapping", "说唱"),
        ("beatboxing", "口技"),
        ("church", "教堂声"),
        ("mosque", "清真寺声"),
        ("temple", "寺庙声"),
        ("bell ringing", "钟鸣"),
        ("wind chime", "风铃"),
        ("gong", "锣"),
        ("mallet", "木槌声"),
        ("wood", "木质声"),
        ("metal", "金属声"),
        ("plastic", "塑料声"),
        ("ceramic", "陶瓷声"),
    ];

    for (needle, zh) in TABLE {
        if contains_word(&key, &needle.to_ascii_lowercase()) {
            return (*zh).to_string();
        }
    }
    english.to_string()
}

/// 整词（或词组）匹配：`needle` 必须落在词边界上。
///
/// 单纯用 `contains` 会让 `cat` 命中 `Category`、`run` 命中 `Burning`，
/// 从而给用户一个语义完全错误的标签。
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let bytes = haystack.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = haystack[from..].find(needle) {
        let start = from + rel;
        let end = start + needle.len();
        // 词边界：前一个字符不是字母数字，后一个字符也不是
        let ok_left = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let ok_right = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if ok_left && ok_right {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// 对 logits 应用 sigmoid，得到每个类别的独立概率。
///
/// **这一步不能省**：AST 输出的是 527 维 **logits**，不是概率。
/// AudioSet 是多标签任务，官方用 `sigmoid` 而非 `softmax`
/// （各类别相互独立，一首曲子可以同时是「音乐」和「人声」）。
/// 不做转换的话全是负值，`scores > 0` 的过滤会把真实命中全部丢掉 ——
/// 这正是我第一版实现里「什么音频都判成人声说话」的原因。
pub fn logits_to_probabilities(logits: &[f32]) -> Vec<f32> {
    logits
        .iter()
        .map(|z| {
            // 数值稳定的 sigmoid
            if *z >= 0.0 {
                1.0 / (1.0 + (-*z).exp())
            } else {
                let e = z.exp();
                e / (1.0 + e)
            }
        })
        .collect()
}

/// 把模型输出 + 类别名转换为标签（可按词汇表翻译）。
///
/// 入参 `logits` 是模型原始输出，函数内部会先做 sigmoid。
pub fn scores_to_tags_with(
    logits: &[f32],
    class_names: &[String],
    top_n: usize,
    low_confidence: f32,
    translate: &dyn Fn(&str) -> String,
) -> Vec<Tag> {
    let scores = logits_to_probabilities(logits);
    let mut indexed: Vec<(usize, f32)> = scores
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, s)| s.is_finite() && *s > 0.0)
        .collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    indexed
        .into_iter()
        .take(top_n * 3) // 跳过重复标签后仍能凑够 top_n
        .filter_map(|(i, score)| {
            let english = class_names.get(i)?;
            if english.is_empty() {
                return None;
            }
            let label = translate(&category_label(english));
            // 同一音频常命中多个近义类别（Gunshot / Machine gun），去重更清爽
            if !seen.insert(label.clone()) {
                return None;
            }
            let mut tag = Tag::new(&label, TagSource::Yamnet, score);
            tag.is_confirmed = score >= low_confidence;
            Some(tag)
        })
        .take(top_n)
        .collect()
}

/// 把模型输出 + 类别名转换为标签（不翻译）。
pub fn scores_to_tags(
    scores: &[f32],
    class_names: &[&str],
    top_n: usize,
    low_confidence: f32,
) -> Vec<Tag> {
    let owned: Vec<String> = class_names.iter().map(|s| s.to_string()).collect();
    scores_to_tags_with(scores, &owned, top_n, low_confidence, &|s| s.to_string())
}

/// 自动标签是否可用：事件分类模型与类别表都就位。
pub fn is_ready(mgr: &ModelManager) -> bool {
    mgr.is_installed(MODEL_NAME) && mgr.is_installed(CONFIG_NAME)
}

/// 若模型可用则推理，返回标签；模型缺失时返回空列表。
///
/// 任何失败都不影响导入链路：调用方只记日志。
pub fn infer_if_available(conn: &Connection, audio: &DecodedAudio) -> Result<Vec<Tag>> {
    let mgr = ModelManager::with_configured_dir(conn);
    if !is_ready(&mgr) {
        return Ok(Vec::new());
    }

    let names = load_class_names(&mgr.model_path(CONFIG_NAME))?;
    let scores = infer_with_path(
        &mgr.model_path(MODEL_NAME),
        &audio.samples,
        audio.sample_rate,
    )?;

    // 类别标签也走词汇表翻译，让「枪声」在游戏主题下显示为「开枪」等说法
    let resolver = crate::utils::config::Settings::load(conn)
        .map(|s| s.tag_resolver())
        .unwrap_or_else(|_| {
            crate::tag_vocab::Resolver::new(Default::default(), Default::default())
        });

    Ok(scores_to_tags_with(
        &scores,
        &names,
        TOP_N,
        LOW_CONFIDENCE,
        &|label| resolver.label(label),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 官方配置里确实存在的几个类别，以及期望的中文。
    #[test]
    fn maps_common_categories_to_chinese() {
        assert_eq!(category_label("Gunshot, gunfire"), "枪声");
        assert_eq!(category_label("Explosion"), "爆炸声");
        assert_eq!(category_label("Click"), "咔哒声");
        assert_eq!(category_label("Music"), "音乐");
        assert_eq!(category_label("Speech"), "人声说话");
        assert_eq!(category_label("Telephone bell ringing"), "铃声");
        assert_eq!(category_label("Alarm clock"), "闹钟");
    }

    /// 表里没有的类别必须原样返回，不能丢。
    #[test]
    fn unknown_category_falls_back_to_english() {
        let unknown = "Some Made Up Category";
        assert_eq!(category_label(unknown), unknown);
    }

    /// logit -> sigmoid 的单调性使排序不变，故这里用 logits 直接构造。
    #[test]
    fn scores_to_tags_picks_top_n_and_sorts() {
        let names = ["Gunshot, gunfire", "Explosion", "Click", "Music", "Silence"];
        let scores = [-2.0f32, 2.2, 0.85, 0.0, -1.4];
        let tags = scores_to_tags(&scores, &names, 3, 0.3);
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0].tag, "爆炸声");
        assert_eq!(tags[1].tag, "咔哒声");
        assert_eq!(tags[2].tag, "音乐");
        assert!(tags[0].confidence > tags[1].confidence);
    }

    #[test]
    fn all_tags_are_ast_sourced() {
        let names = ["Gunshot, gunfire", "Explosion", "Click"];
        let tags = scores_to_tags(&[0.5, 0.4, 0.3], &names, 3, 0.3);
        assert!(tags.iter().all(|t| t.source == TagSource::Yamnet));
    }

    /// 置信度门槛比较的是 **sigmoid 之后**的概率。
    #[test]
    fn low_confidence_is_marked_unconfirmed() {
        let names = ["Gunshot, gunfire", "Explosion"];
        // logit 2.0 -> p≈0.881（已确认）；logit -2.0 -> p≈0.119（待确认）
        let tags = scores_to_tags(&[2.0, -2.0], &names, 2, 0.3);
        assert!(tags[0].is_confirmed, "高置信度应视为已确认");
        assert!(!tags[1].is_confirmed, "低置信度应标记为待确认");
    }

    /// sigmoid 的数值稳定性：极端 logit 不应溢出。
    #[test]
    fn sigmoid_is_numerically_stable() {
        let p = logits_to_probabilities(&[100.0, -100.0, 0.0, f32::NAN]);
        assert!((p[0] - 1.0).abs() < 1e-6, "大正 logit 应接近 1");
        assert!(p[1] < 1e-6, "大负 logit 应接近 0");
        assert!((p[2] - 0.5).abs() < 1e-6, "logit 0 应为 0.5");
        assert!(p[3].is_nan());
    }

    /// 近义类别应去重，避免同一音频出现「枪声 / 机枪声 / 连发枪声」三条。
    #[test]
    fn near_duplicate_labels_are_deduplicated() {
        let names = ["Gunshot, gunfire", "Machine gun", "Fusillade", "Explosion"];
        let scores = [2.0f32, 1.6, 1.2, 0.8];
        let tags = scores_to_tags(&scores, &names, 4, 0.3);
        let labels: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
        assert_eq!(
            labels,
            vec!["枪声", "机枪声", "连发枪声", "爆炸声"],
            "不同中文标签不应被判为重复"
        );

        // 真正重复的情况：同类名出现两次
        let dup = ["Gunshot, gunfire", "Gunshot, gunfire", "Explosion"];
        let tags2 = scores_to_tags(&[2.0f32, 1.5, 1.0], &dup, 5, 0.3);
        assert_eq!(tags2.len(), 2, "重复标签应被去掉");
    }

    #[test]
    fn negative_and_nan_scores_are_ignored() {
        let names = ["Gunshot, gunfire", "Explosion", "Click"];
        // NaN logit 经 sigmoid 仍是 NaN，必须被过滤掉而不是污染标签
        let tags = scores_to_tags(&[f32::NAN, -12.0, 0.5], &names, 5, 0.3);
        assert!(
            tags.iter().all(|t| t.confidence.is_finite()),
            "不应出现 NaN 置信度"
        );
        assert!(tags.iter().all(|t| t.tag != "枪声"), "NaN 类别不应入选");
        // 最高 logit 应对应第一个标签
        assert_eq!(tags[0].tag, "咔哒声");
    }

    #[test]
    fn missing_model_returns_empty_without_error() {
        if std::env::var("AAM_MODELS_DIR").is_ok() {
            eprintln!("跳过：AAM_MODELS_DIR 已设置");
            return;
        }
        let conn = crate::db::open_in_memory().unwrap();
        let audio = DecodedAudio {
            samples: vec![0.0; 1600],
            sample_rate: 16_000,
            channels: 1,
            duration: 0.1,
            bitrate: None,
            format: Some("wav".into()),
            truncated: false,
        };
        assert!(infer_if_available(&conn, &audio).unwrap().is_empty());
    }

    #[test]
    fn missing_config_reports_readable_error() {
        let r = load_class_names(std::path::Path::new("C:/nope/config.json"));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("读取类别配置失败"));
    }
}
