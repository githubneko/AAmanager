//! 标签词汇表：把同一份「声学事实」用不同职业的语言表达。
//!
//! ## 为什么需要
//!
//! 规则引擎产出的是**声学描述**（「低频」「沉闷」「尖锐」）。对音效设计师这是
//! 精确的，但对游戏开发者、剪辑师就太抽象了 —— 他们要的是「这是爆炸声」。
//!
//! ## 设计
//!
//! * 数据库里始终存**规范标签**（canonical，即规则引擎那一套）。它是稳定的事实层，
//!   不受用户切换词汇表影响。
//! * 展示时经过词汇表映射成该职业习惯的说法。切换词汇表只是换了一层「翻译」，
//!   不需要迁移数据，也不会因为换词汇表而丢掉用户自己的标签。
//! * 用户标签（`source = user`）**永远不翻译** —— 那是用户自己写的词。
//! * 搜索时做双向展开：用户搜「激光」也能命中规范标签「尖锐」的音频。
//! * 仅影响展示、搜索与自动备注，**不影响 ML 模型与向量**（它们吃的是原始音频）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 词汇表标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagVocab {
    /// 游戏创作者：强调「这是什么音效」，偏事件名。
    #[default]
    Game,
    /// 剪辑 / 视频创作者：强调「听感与用途」。
    Video,
    /// 音乐制作人：强调专业术语（频段、时域、频谱特征）。
    Music,
    /// 自定义：用户在默认术语基础上自行改名。
    Custom,
}

impl TagVocab {
    /// 数据库中的标识字符串。
    pub fn as_db(&self) -> &'static str {
        match self {
            TagVocab::Game => "game",
            TagVocab::Video => "video",
            TagVocab::Music => "music",
            TagVocab::Custom => "custom",
        }
    }

    /// 从数据库字符串解析，未知值回退到默认。
    pub fn from_db(s: &str) -> Self {
        match s {
            "video" => TagVocab::Video,
            "music" => TagVocab::Music,
            "custom" => TagVocab::Custom,
            _ => TagVocab::Game,
        }
    }

    /// 全部词汇表（用于前端渲染选择器）。
    pub fn all() -> &'static [TagVocab] {
        &[
            TagVocab::Game,
            TagVocab::Video,
            TagVocab::Music,
            TagVocab::Custom,
        ]
    }
}

/// 一个规范标签在各词汇表下的说法。
struct VocabEntry {
    /// 规范标签（数据库里存的值）。
    canonical: &'static str,
    game: &'static str,
    video: &'static str,
    music: &'static str,
}

/// 词汇映射表。
///
/// 每一行的四个值必须同义 —— 它们描述的是同一类声音。规范标签同时也是
/// 「自定义」词汇表的初始值（用户没改过的项就显示规范词）。
const VOCAB: &[VocabEntry] = &[
    // ---- 时长 ----
    VocabEntry {
        canonical: "短促",
        game: "瞬间音效",
        video: "短音",
        music: "瞬态短音",
    },
    VocabEntry {
        canonical: "中等时长",
        game: "短音效",
        video: "常规长度",
        music: "中段素材",
    },
    VocabEntry {
        canonical: "长音",
        game: "环境循环",
        video: "长镜头铺底",
        music: "持续音",
    },
    // ---- 频谱亮度 ----
    VocabEntry {
        canonical: "尖锐",
        game: "激光/枪击",
        video: "高频刺耳",
        music: "高Q/明亮瞬态",
    },
    VocabEntry {
        canonical: "高频",
        game: "高频特效",
        video: "高频",
        music: "高频段",
    },
    VocabEntry {
        canonical: "沉闷",
        game: "爆炸/轰鸣",
        video: "低沉轰响",
        music: "低频厚重",
    },
    VocabEntry {
        canonical: "低频",
        game: "低频冲击",
        video: "低频",
        music: "低频段",
    },
    VocabEntry {
        canonical: "明亮",
        game: "闪亮特效",
        video: "明亮",
        music: "明亮（高滚降）",
    },
    VocabEntry {
        canonical: "暗淡",
        game: "闷响",
        video: "发闷",
        music: "暗（低滚降）",
    },
    // ---- 时域 / 能量 ----
    VocabEntry {
        canonical: "瞬态",
        game: "爆发起音",
        video: "硬切起音",
        music: "瞬态fast",
    },
    VocabEntry {
        canonical: "缓入",
        game: "渐强起手",
        video: "渐入",
        music: "慢起音",
    },
    VocabEntry {
        canonical: "响度大",
        game: "高冲击",
        video: "响度高",
        music: "高RMS",
    },
    VocabEntry {
        canonical: "安静",
        game: "微弱音",
        video: "轻音",
        music: "低RMS",
    },
    // ---- 噪声性 ----
    VocabEntry {
        canonical: "噪声感",
        game: "撕裂/摩擦",
        video: "沙沙声",
        music: "高ZCR噪声",
    },
    VocabEntry {
        canonical: "含噪",
        game: "带噪特效",
        video: "略带底噪",
        music: "含噪声成分",
    },
    VocabEntry {
        canonical: "宽带噪声",
        game: "爆炸/冲击",
        video: "宽频噪声",
        music: "宽带噪声",
    },
    VocabEntry {
        canonical: "乐音感",
        game: "音调音效",
        video: "有音高",
        music: "有明确音高",
    },
    VocabEntry {
        canonical: "静音",
        game: "空白素材",
        video: "无内容",
        music: "静音",
    },
];

/// 规范标签 -> 该词汇表下的展示名。
fn label_for(
    canonical: &str,
    vocab: TagVocab,
    overrides: &HashMap<String, String>,
) -> Option<String> {
    // 自定义覆盖优先（仅自定义词汇表生效由调用方决定）
    if let Some(v) = overrides.get(canonical) {
        return Some(v.clone());
    }
    let entry = VOCAB.iter().find(|e| e.canonical == canonical)?;
    Some(
        match vocab {
            TagVocab::Game => entry.game,
            TagVocab::Video => entry.video,
            TagVocab::Music => entry.music,
            // 自定义词汇表以规范词为默认值，用户改过的项由 overrides 覆盖
            TagVocab::Custom => entry.canonical,
        }
        .to_string(),
    )
}

/// 把展示名登记进反向索引（整串 + 分隔符片段 + 中文前缀 + ASCII 词）。
///
/// ## 为什么用「前缀」而不是「二元组」
///
/// 中文没有词边界，用户在搜索框里边打字边搜，天然期望**前缀命中**：
/// 把「沉闷」改名为「闷雷滚滚」后，敲「闷雷」就该看到结果。
///
/// 但也不能用 CJK 二元组做索引 —— 那会产生大量巧合：
/// 游戏词汇表里「暗淡」的展示名是「闷响」，二元组会从中切出「沉闷」，
/// 于是搜「沉闷」会莫名多出带「暗淡」标签的无关音频（实测到的误召回）。
///
/// 前缀既满足「边打边搜」，又不会跨词巧合。
/// 单字前缀不索引：噪声太大（任何含该字的标签都会被召回）。
fn push_parts(reverse: &mut HashMap<String, String>, label: &str, canonical: &str) {
    reverse
        .entry(label.to_lowercase())
        .or_insert_with(|| canonical.to_string());
    for part in label.split(['/', '|', '、', ',', '，', ' ', '(', ')', '（', '）']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        reverse
            .entry(part.to_lowercase())
            .or_insert_with(|| canonical.to_string());
        if part.is_ascii() {
            // 英文按词边界拆（"low freq" -> low / freq）
            for w in part.split(|c: char| !c.is_alphanumeric()) {
                if !w.is_empty() {
                    reverse
                        .entry(w.to_lowercase())
                        .or_insert_with(|| canonical.to_string());
                }
            }
        } else {
            // 中文：索引长度 >= 2 的所有前缀
            let mut cur = String::new();
            let mut runs: Vec<String> = Vec::new();
            for c in part.chars() {
                if crate::text::is_cjk(c) {
                    cur.push(c);
                } else if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            }
            if !cur.is_empty() {
                runs.push(cur);
            }
            for run in runs {
                let chars: Vec<char> = run.chars().collect();
                for len in 2..=chars.len() {
                    let prefix: String = chars[..len].iter().collect();
                    reverse
                        .entry(prefix.to_lowercase())
                        .or_insert_with(|| canonical.to_string());
                }
            }
        }
    }
}

/// 词汇表解析器。
///
/// 一次性构建规范名 -> 展示名的映射，避免在列表渲染时对每个标签做线性查找。
pub struct Resolver {
    vocab: TagVocab,
    /// 规范标签 -> 展示名
    forward: HashMap<String, String>,
    /// 展示名（小写）-> 规范标签，用于搜索展开
    reverse: HashMap<String, String>,
}

impl Resolver {
    /// 构建解析器。`overrides` 是自定义词汇表的重命名表。
    pub fn new(vocab: TagVocab, overrides: HashMap<String, String>) -> Self {
        let mut forward = HashMap::new();
        let mut reverse = HashMap::new();

        for entry in VOCAB {
            if let Some(label) = label_for(entry.canonical, vocab, &overrides) {
                // 展示名常是复合词（「爆炸/轰鸣」「爆炸/冲击」）。只索引整串的话，
                // 用户搜「爆炸」会落空 —— 而「爆炸」恰恰是词表里最显眼的那个词。
                // 具体拆法见 `push_parts`（按分隔符，不用 CJK 二元组）。
                push_parts(&mut reverse, &label, entry.canonical);

                // 规范名本身也能搜到（用户可能直接搜「低频」）
                reverse
                    .entry(entry.canonical.to_lowercase())
                    .or_insert_with(|| entry.canonical.to_string());
                forward.insert(entry.canonical.to_string(), label);
            }
        }

        // 自定义改名同样要按片段可搜：用户把「沉闷」改成「闷雷滚滚」后，
        // 搜「闷雷」应当命中。overrides 里的名字不在 VOCAB 内，
        // 因此需要单独再补进反向表。
        //
        // 同样只按分隔符拆完整片段 + 纯 ASCII 词，不用 CJK 二元组
        // （理由见上：二元组会引入巧合映射）。
        for (canonical, custom) in &overrides {
            let custom = custom.trim();
            if custom.is_empty() {
                continue;
            }
            push_parts(&mut reverse, custom, canonical);
        }

        Self {
            vocab,
            forward,
            reverse,
        }
    }

    /// 当前词汇表。
    pub fn vocab(&self) -> TagVocab {
        self.vocab
    }

    /// 把规范标签翻译成展示名。表里没有的标签原样返回
    /// （例如用户标签、模型类别标签）。
    pub fn label(&self, canonical: &str) -> String {
        self.forward
            .get(canonical)
            .cloned()
            .unwrap_or_else(|| canonical.to_string())
    }

    /// 把用户输入的文本展开为「可能的规范标签」列表（用于搜索）。
    ///
    /// 返回空表示这段输入不是词汇表里的说法，按普通关键词处理。
    ///
    /// ## 为什么要切词再查
    ///
    /// 展示名常是复合词（如「爆炸/轰鸣」）。若只做整串查表，用户搜「爆炸」
    /// 或「轰鸣」都落空 —— 那就成了"只有外表变了、搜索却对不上"。
    /// 因此这里把输入与**展示名本身**都按 [`crate::text::tokenize`] 切开，
    /// 于是下面这些写法都能命中同一条规范标签「沉闷」：
    ///
    /// * 展示名全串：`爆炸/轰鸣`
    /// * 展示名的任一片段：`爆炸`、`轰鸣`
    /// * 规范名本身：`沉闷`
    /// * 用户自定义的名称：改叫什么就按什么搜（含其片段）
    /// * 夹在句子里：`沉闷的爆炸` → 切出 `沉闷`、`爆炸`
    pub fn expand(&self, term: &str) -> Vec<String> {
        let term = term.trim();
        if term.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        let push = |canonical: &str, out: &mut Vec<String>| {
            if !out.iter().any(|c| c == canonical) {
                out.push(canonical.to_string());
            }
        };

        // 1) 整串直接查表（覆盖纯 ASCII 词与恰好等于展示名/规范名的情况）
        if let Some(canonical) = self.reverse.get(&term.to_lowercase()) {
            push(canonical, &mut out);
        }

        // 2) 切词后逐个查（覆盖 CJK 复合展示名的片段）
        //
        //    注意：中文会被切成单字，因此规范名「低频」的**整串**必须单独保留，
        //    否则退化成搜「低」+「频」，两个单字都命中不了「低频」这个标签，
        //    反而把原本精确的查询拆坏了。
        for word in crate::text::tokenize(term) {
            if let Some(canonical) = self.reverse.get(&word.to_lowercase()) {
                push(canonical, &mut out);
            }
        }

        // 3) 反向覆盖：输入里包含某个**完整展示名**时也算命中。
        //    例如自定义把「沉闷」改名为「闷雷」，用户搜「沉闷的闷雷声」，
        //    切词未必能还原「闷雷」，但整串包含关系能兜住。
        let lower = term.to_lowercase();
        for (label, canonical) in &self.reverse {
            if !label.is_empty() && lower.contains(label.as_str()) {
                push(canonical, &mut out);
            }
        }

        out
    }

    /// 把一段查询串展开为额外的检索词（不含原词）。
    ///
    /// 例：词汇表为「游戏」时，输入「沉闷的爆炸」会展开出规范标签
    /// `["沉闷"]`，于是搜索既能命中规范标签，也能命中用户自己写的词。
    ///
    /// 注意这里对**整段文本**做展开（而不是先按空格切）：
    /// 中文没有空格，切词交给 [`expand`](Self::expand) 内部处理。
    pub fn expand_query(&self, text: &str) -> Vec<String> {
        self.expand(text)
    }

    /// 词汇表内容快照（供前端展示「规范词 -> 当前说法」对照表）。
    pub fn table(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = VOCAB
            .iter()
            .map(|e| (e.canonical.to_string(), self.label(e.canonical)))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }
}

/// 该标签是否可被词汇表翻译。
///
/// 只有规则引擎产出的声学描述才翻译；用户标签与模型类别标签保持原样。
pub fn is_translatable(source: &str) -> bool {
    matches!(source, "rule")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(vocab: TagVocab) -> Resolver {
        Resolver::new(vocab, HashMap::new())
    }

    #[test]
    fn every_vocabulary_translates_all_canonical_tags() {
        for vocab in TagVocab::all() {
            let r = resolver(*vocab);
            for entry in VOCAB {
                let label = r.label(entry.canonical);
                assert!(
                    !label.is_empty(),
                    "{:?} 下 {} 的展示名不应为空",
                    vocab,
                    entry.canonical
                );
            }
        }
    }

    #[test]
    fn game_vocabulary_uses_event_words() {
        let r = resolver(TagVocab::Game);
        assert_eq!(r.label("沉闷"), "爆炸/轰鸣");
        assert_eq!(r.label("尖锐"), "激光/枪击");
        assert_eq!(r.label("宽带噪声"), "爆炸/冲击");
    }

    #[test]
    fn music_vocabulary_uses_technical_terms() {
        let r = resolver(TagVocab::Music);
        assert_eq!(r.label("尖锐"), "高Q/明亮瞬态");
        assert_eq!(r.label("瞬态"), "瞬态fast");
        assert_eq!(r.label("缓入"), "慢起音");
        assert_eq!(r.label("响度大"), "高RMS");
        assert_eq!(r.label("低频"), "低频段");
    }

    #[test]
    fn video_vocabulary_is_perceptual() {
        let r = resolver(TagVocab::Video);
        assert_eq!(r.label("沉闷"), "低沉轰响");
        assert_eq!(r.label("短促"), "短音");
        assert_eq!(r.label("长音"), "长镜头铺底");
    }

    #[test]
    fn custom_vocabulary_defaults_to_canonical_and_honours_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("沉闷".to_string(), "我的爆炸".to_string());
        let r = Resolver::new(TagVocab::Custom, overrides);
        assert_eq!(r.label("沉闷"), "我的爆炸", "自定义覆盖应生效");
        assert_eq!(r.label("尖锐"), "尖锐", "未覆盖的项用规范词");
    }

    #[test]
    fn unknown_tags_pass_through_untouched() {
        let r = resolver(TagVocab::Music);
        assert_eq!(r.label("我的素材"), "我的素材");
        assert_eq!(r.label("Gunshot"), "Gunshot");
    }

    /// 展示名的**片段**必须可搜。
    ///
    /// 游戏词汇表里「沉闷」显示为「爆炸/轰鸣」。如果只索引整串，
    /// 用户搜「爆炸」会落空 —— 而「爆炸」恰恰是词表里最显眼的词。
    /// 这正是「只有外表不同、搜索对不上」的根因。
    #[test]
    fn composite_display_label_parts_are_searchable() {
        let r = resolver(TagVocab::Game);
        // 整串
        assert_eq!(r.expand("爆炸/轰鸣"), vec!["沉闷".to_string()]);
        // 片段
        assert_eq!(r.expand("爆炸"), vec!["沉闷".to_string()]);
        assert_eq!(r.expand("轰鸣"), vec!["沉闷".to_string()]);
        // 夹在句子里
        assert!(r.expand("沉闷的爆炸").contains(&"沉闷".to_string()));
    }

    /// 用户自定义的名称（含其片段）也要能搜到。
    #[test]
    fn custom_override_names_are_searchable() {
        let mut ov = HashMap::new();
        ov.insert("沉闷".to_string(), "闷雷滚滚".to_string());
        ov.insert("尖锐".to_string(), "laser zap".to_string());
        let r = Resolver::new(TagVocab::Custom, ov);

        // 整串
        assert_eq!(r.expand("闷雷滚滚"), vec!["沉闷".to_string()]);
        // 片段（中文二元组）
        assert_eq!(r.expand("闷雷"), vec!["沉闷".to_string()]);
        // 纯英文自定义名
        assert_eq!(r.expand("laser zap"), vec!["尖锐".to_string()]);
        assert_eq!(r.expand("laser"), vec!["尖锐".to_string()]);
        // 展示也走自定义名
        assert_eq!(r.label("沉闷"), "闷雷滚滚");
    }

    #[test]
    fn search_expansion_maps_display_word_to_canonical() {
        let r = resolver(TagVocab::Game);
        // 游戏词汇表下用户会搜「激光」，应展开到规范标签「尖锐」
        let expanded = r.expand("激光/枪击");
        assert_eq!(expanded, vec!["尖锐".to_string()]);

        // 直接搜规范词也应能被识别
        let expanded = r.expand("低频");
        assert!(expanded.contains(&"低频".to_string()));
    }

    #[test]
    fn expand_query_handles_multiple_words_and_dedupes() {
        let r = resolver(TagVocab::Game);
        let extra = r.expand_query("爆炸/轰鸣 激光/枪击 未知词");
        assert!(extra.contains(&"沉闷".to_string()), "{extra:?}");
        assert!(extra.contains(&"尖锐".to_string()), "{extra:?}");
        // 未知词不产生展开
        assert!(!extra.contains(&"未知词".to_string()));
        // 无重复
        let mut sorted = extra.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), extra.len());

        // 重复输入只展开一次
        let extra2 = r.expand_query("激光/枪击 激光/枪击");
        assert_eq!(extra2, vec!["尖锐".to_string()]);
    }

    #[test]
    fn expand_query_on_empty_input_is_empty() {
        let r = resolver(TagVocab::Game);
        assert!(r.expand_query("").is_empty());
        assert!(r.expand_query("   ").is_empty());
    }

    #[test]
    fn table_covers_every_entry() {
        let r = resolver(TagVocab::Game);
        let table = r.table();
        assert_eq!(table.len(), VOCAB.len());
        // 表里每个规范名都能查到
        for (canonical, _) in &table {
            assert!(!r.label(canonical).is_empty());
        }
    }

    #[test]
    fn vocab_roundtrips_through_db_strings() {
        for v in TagVocab::all() {
            assert_eq!(TagVocab::from_db(v.as_db()), *v);
        }
        // 未知值回退到默认
        assert_eq!(TagVocab::from_db("nonsense"), TagVocab::default());
    }

    #[test]
    fn only_rule_tags_are_translatable() {
        assert!(is_translatable("rule"));
        assert!(!is_translatable("user"), "用户标签不应被翻译");
        assert!(!is_translatable("yamnet"));
        assert!(!is_translatable("clap"));
        assert!(!is_translatable("inherited"));
    }

    /// 同一类声音在各词汇表下必须是**不同的**说法，否则词汇表没有意义。
    #[test]
    fn vocabularies_actually_differ() {
        let g = resolver(TagVocab::Game);
        let v = resolver(TagVocab::Video);
        let m = resolver(TagVocab::Music);
        let mut differing = 0;
        for entry in VOCAB {
            let (a, b, c) = (
                g.label(entry.canonical),
                v.label(entry.canonical),
                m.label(entry.canonical),
            );
            if !(a == b && b == c) {
                differing += 1;
            }
        }
        assert_eq!(
            differing,
            VOCAB.len(),
            "每个规范标签在三套词汇表下都应有不同说法"
        );
    }
}
