//! 查询串切词。
//!
//! ## 为什么需要它
//!
//! 原来切词只按空格（`split_whitespace`），对中文是致命的：
//! 用户输入「沉闷的爆炸」是一个**连续串**，而库里存的是两个独立标签
//! 「沉闷」和「爆炸」，FTS5 的短语匹配与 `LIKE %沉闷的爆炸%` 都命中不了，
//! 于是"明明有标签却搜不出来"。
//!
//! ## 切词策略
//!
//! 对每个空格分隔的片段：
//!
//! * **ASCII 词**（英文、数字、下划线）整体保留 —— `gunshot` 不该被拆成
//!   单个字母，`ui_click` 也一样；
//! * **CJK 串**同时产出**单字**与**相邻二元组**：
//!   「沉闷的爆炸」→ 单字 `沉闷的爆炸` + 二元组 `沉闷`、`闷的`、`的爆`、`爆炸`。
//!   二元组正好命中自带标签，单字则能命中标签中的单个字
//!   （官方 AudioSet 类别里就有「鼓」「枪」这类单字标签）。
//!
//! 单字会带来一些噪声（比如「的」），但检索是取候选集的**并集**，
//! 噪声只会让候选变多，不会让正确结果消失；而少了单字，
//! 单字标签就永远搜不到。
//!
//! 未匹配的片段（如「闷的」）不会命中任何数据，`AND` 模式下因此必须
//! 依赖二元组提供的正确命中 —— 这也是二元组不可省的原因。

/// 判断是否为 CJK（中日韩）字符。
///
/// 覆盖常用区间即可：汉字、日文假名、韩文音节、CJK 标点。
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF      // 平假名 / 片假名
        | 0x3400..=0x4DBF    // 扩展 A
        | 0x4E00..=0x9FFF    // 基本汉字
        | 0xF900..=0xFAFF    // 兼容汉字
        | 0xAC00..=0xD7AF    // 韩文音节
        | 0x20000..=0x2FA1F  // 扩展 B+
    )
}

/// 把查询串切成检索词。
///
/// 返回值已去重、去空、按出现顺序排列。
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for chunk in text.split_whitespace() {
        let chars: Vec<char> = chunk.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if is_cjk(c) {
                // 收集连续的 CJK 串
                let start = i;
                while i < chars.len() && is_cjk(chars[i]) {
                    i += 1;
                }
                let run: Vec<char> = chars[start..i].to_vec();
                // 单字
                for ch in &run {
                    push_unique(&mut out, ch.to_string());
                }
                // 相邻二元组
                for w in run.windows(2) {
                    push_unique(&mut out, w.iter().collect::<String>());
                }
            } else if c.is_alphanumeric() {
                // ASCII / 其它字母数字整体保留
                let start = i;
                while i < chars.len() && chars[i].is_alphanumeric() && !is_cjk(chars[i]) {
                    i += 1;
                }
                push_unique(&mut out, chars[start..i].iter().collect::<String>());
            } else {
                // 标点与分隔符：丢弃（用户可能从词表里连符号一起复制）
                i += 1;
            }
        }
    }

    out
}

/// 追加去重（大小写不敏感比较，但保留原始大小写）。
fn push_unique(out: &mut Vec<String>, s: String) {
    if s.is_empty() {
        return;
    }
    if out.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        return;
    }
    out.push(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_text_yields_unigrams_and_bigrams() {
        let t = tokenize("沉闷的爆炸");
        // 单字
        for c in ["沉", "闷", "的", "爆", "炸"] {
            assert!(t.contains(&c.to_string()), "缺少单字 {c}: {t:?}");
        }
        // 关键：二元组必须包含自带标签
        assert!(t.contains(&"沉闷".to_string()), "缺少「沉闷」: {t:?}");
        assert!(t.contains(&"爆炸".to_string()), "缺少「爆炸」: {t:?}");
        // 不该出现跨不过去的整串（那正是搜不到的原因）
        assert!(
            !t.contains(&"沉闷的爆炸".to_string()),
            "不应把整串当词: {t:?}"
        );
    }

    #[test]
    fn ascii_words_stay_whole() {
        let t = tokenize("gunshot ui_click 440hz");
        assert!(t.contains(&"gunshot".to_string()), "{t:?}");
        // 下划线是分隔符，故拆成 ui / click
        assert!(
            t.contains(&"ui".to_string()) && t.contains(&"click".to_string()),
            "{t:?}"
        );
        assert!(t.contains(&"440hz".to_string()), "{t:?}");
        assert!(!t.contains(&"gunsho".to_string()), "不应拆开英文: {t:?}");
    }

    #[test]
    fn mixed_cjk_and_ascii() {
        let t = tokenize("沉闷explosion声");
        assert!(t.contains(&"explosion".to_string()), "{t:?}");
        assert!(t.contains(&"沉闷".to_string()), "{t:?}");
        assert!(t.contains(&"声".to_string()), "{t:?}");
    }

    #[test]
    fn vocabulary_separators_are_dropped() {
        // 用户可能直接从词表复制「爆炸/轰鸣」
        let t = tokenize("爆炸/轰鸣");
        assert!(t.contains(&"爆炸".to_string()), "{t:?}");
        assert!(t.contains(&"轰鸣".to_string()), "{t:?}");
        assert!(!t.contains(&"/".to_string()), "{t:?}");
    }

    #[test]
    fn single_cjk_char_is_kept() {
        // 官方类别里存在单字标签
        assert_eq!(tokenize("鼓"), vec!["鼓".to_string()]);
        assert_eq!(tokenize("枪"), vec!["枪".to_string()]);
    }

    #[test]
    fn deduplicates_repeated_terms() {
        let t = tokenize("枪声 枪声");
        assert_eq!(t.iter().filter(|x| *x == "枪声").count(), 1, "{t:?}");
    }

    #[test]
    fn empty_and_punctuation_only_input() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("   ").is_empty());
        assert!(tokenize("///").is_empty());
        assert!(tokenize("，。！").is_empty());
    }

    #[test]
    fn english_is_case_insensitively_deduped() {
        let t = tokenize("Gunshot gunshot");
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(t[0], "Gunshot", "应保留首次出现的大小写");
    }

    #[test]
    fn korean_and_japanese_are_handled() {
        // 韩文音节
        let t = tokenize("음악");
        assert!(t.contains(&"음".to_string()), "{t:?}");
        assert!(t.contains(&"음악".to_string()), "{t:?}");
        // 片假名
        let t2 = tokenize("ドラム");
        assert!(t2.contains(&"ド".to_string()), "{t2:?}");
    }
}
