//! CLAP 文本编码器（纯 Rust）：把自然语言查询编码为 512 维向量。
//!
//! ## 为什么不再用 Python 侧车
//!
//! 早期版本通过 `resources/clap_encode.py` 调用本机 Python + onnxruntime。
//! 缺点明显：给最终用户引入 Python 运行时依赖，且每次查询都要 spawn 一个进程。
//! 现在改用 `tokenizers` crate（Rust 原生 RoBERTa BPE）+ `ort`，
//! **不依赖任何外部运行时**，查询在进程内完成。
//!
//! ## 分词约定
//!
//! 与官方一致：RoBERTa BPE，`<s>` / `</s>` 包裹，截断到 [`MAX_TOKENS`]（77），
//! 用 `<pad>`（id 1）补齐。输入 `input_ids` 形状 `[1, 77]`，输出 `text_embeds`。
//!
//! ## 跨模态对齐的现状（重要）
//!
//! 文本塔与音频塔**各自**都能产出结构良好的向量，但在
//! `Xenova/clap-htsat-unfused` 的 ONNX 导出上两者**不在同一可比空间**：
//! 实测同一音频换不同提示词时 logits 变化很大，换音频时几乎不变。
//! 该结论经官方完整模型（`model.onnx` 直出 `logits_per_audio`）与
//! PyTorch 参考双重确认，详见 README「语义搜索」一节。
//!
//! 因此本模块只负责「正确地把文本变成向量」；是否启用语义检索由调用方决定。
//! 音频向量链路已与 PyTorch **逐位一致**（余弦 1.000000），
//! 换用可用的音频塔导出后无需改动本文件。

use crate::error::{AppError, Result};
use crate::ml::model_manager::ModelManager;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use tokenizers::Tokenizer;

/// 模型逻辑名。
pub const MODEL_NAME: &str = "clap_text.onnx";

/// 分词器文件名。
pub const TOKENIZER_NAME: &str = "tokenizer.json";

/// 输出向量维度。
pub const EMBEDDING_DIM: usize = 512;

/// 模型版本标识。
pub const MODEL_VERSION: &str = "clap-htsat-unfused-onnx-v1";

/// 最大 token 数（CLAP 文本塔的上下文长度）。
pub const MAX_TOKENS: usize = 77;

/// `<pad>` 的 id（RoBERTa 约定）。
const PAD_ID: i64 = 1;

/// 缓存已加载的分词器（键 = 分词器文件路径）。
fn tokenizer_cache() -> &'static Mutex<Option<(String, Tokenizer)>> {
    static CACHE: OnceLock<Mutex<Option<(String, Tokenizer)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 从磁盘加载（并缓存）分词器。
pub fn load_tokenizer(path: &Path) -> Result<Tokenizer> {
    if !path.is_file() {
        return Err(AppError::NotFound(format!(
            "分词器不存在: {}",
            path.display()
        )));
    }
    let key = path.to_string_lossy().to_string();

    {
        let guard = tokenizer_cache()
            .lock()
            .map_err(|_| AppError::Audio("分词器缓存锁已损坏".into()))?;
        if let Some((cached_key, tok)) = guard.as_ref() {
            if *cached_key == key {
                return Ok(tok.clone());
            }
        }
    }

    let mut tok =
        Tokenizer::from_file(path).map_err(|e| AppError::Audio(format!("加载分词器失败: {e}")))?;
    tok.with_truncation(Some(tokenizers::TruncationParams {
        max_length: MAX_TOKENS,
        ..Default::default()
    }))
    .map_err(|e| AppError::Audio(format!("配置截断失败: {e}")))?;
    tok.with_padding(Some(tokenizers::PaddingParams {
        strategy: tokenizers::PaddingStrategy::Fixed(MAX_TOKENS),
        pad_id: PAD_ID as u32,
        pad_token: "<pad>".to_string(),
        ..Default::default()
    }));

    let mut guard = tokenizer_cache()
        .lock()
        .map_err(|_| AppError::Audio("分词器缓存锁已损坏".into()))?;
    *guard = Some((key, tok.clone()));
    Ok(tok)
}

/// 把查询串编码成 `[1, 77]` 的 input_ids。
pub fn encode_ids(tokenizer: &Tokenizer, text: &str) -> Result<Vec<i64>> {
    let enc = tokenizer
        .encode(text, true)
        .map_err(|e| AppError::Audio(format!("分词失败: {e}")))?;
    let ids: Vec<i64> = enc.get_ids().iter().map(|v| *v as i64).collect();
    if ids.len() != MAX_TOKENS {
        return Err(AppError::Audio(format!(
            "分词长度异常: 期望 {MAX_TOKENS}，实际 {}",
            ids.len()
        )));
    }
    Ok(ids)
}

/// 对已分词的 id 做一次前向，返回归一化后的 512 维向量。
pub fn embed_ids(model_path: &Path, ids: &[i64]) -> Result<Vec<f32>> {
    if ids.len() != MAX_TOKENS {
        return Err(AppError::Invalid(format!(
            "input_ids 长度错误: 期望 {MAX_TOKENS}，实际 {}",
            ids.len()
        )));
    }

    let input = ort::value::Tensor::from_array(([1i64, MAX_TOKENS as i64], ids.to_vec()))
        .map_err(|e| AppError::Audio(format!("构造输入张量失败: {e}")))?;

    crate::ml::inference::global().with_session(model_path, |session| {
        let outputs = session
            .run(ort::inputs!["input_ids" => input])
            .map_err(|e| AppError::Audio(format!("CLAP 文本推理失败: {e}")))?;

        let (shape, data) = outputs
            .get("text_embeds")
            .ok_or_else(|| AppError::Audio("模型输出缺少 text_embeds".into()))?
            .try_extract_tensor::<f32>()
            .map_err(|e| AppError::Audio(format!("解析文本向量失败: {e}")))?;

        let vec: Vec<f32> = data.to_vec();
        if vec.len() != EMBEDDING_DIM {
            return Err(AppError::Audio(format!(
                "文本向量维度异常: 期望 {EMBEDDING_DIM}，实际 {}（shape {shape:?}）",
                vec.len()
            )));
        }
        Ok(crate::ml::clap_audio::normalize(vec))
    })
}

/// 若模型可用则编码文本，否则返回 `None`。
///
/// 命中缓存时不会走模型，因此即使模型未安装，已缓存的查询词依然可用。
/// 任何一步失败都返回 `None`，由调用方降级为关键词搜索。
pub fn encode_if_available(conn: &Connection, text: &str) -> Result<Option<Vec<f32>>> {
    let query = normalize_query(text);
    if query.is_empty() {
        return Ok(None);
    }

    // 1. 缓存优先
    if let Some(cached) = crate::db::vector_repo::get_text_embedding(conn, &query, MODEL_VERSION)? {
        return Ok(Some(cached));
    }

    // 2. 模型与分词器必须都在
    let mgr = ModelManager::with_configured_dir(conn);
    if !mgr.is_installed(MODEL_NAME) || !mgr.is_installed(TOKENIZER_NAME) {
        return Ok(None);
    }

    // 3. 进程内推理
    let ids = match load_tokenizer(&mgr.model_path(TOKENIZER_NAME))
        .and_then(|t| encode_ids(&t, &query))
    {
        Ok(v) => v,
        Err(e) => {
            log::warn!("分词不可用，语义检索降级: {e}");
            return Ok(None);
        }
    };
    let vec = match embed_ids(&mgr.model_path(MODEL_NAME), &ids) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("文本编码失败，语义检索降级: {e}");
            return Ok(None);
        }
    };

    // 4. 写缓存（失败不影响本次结果）
    if let Err(e) = crate::db::vector_repo::cache_text_embedding(conn, &query, &vec, MODEL_VERSION)
    {
        log::warn!("写入文本向量缓存失败: {e}");
    }
    Ok(Some(vec))
}

/// 语义检索是否可用：文本模型、音频模型、分词器都就位。
pub fn is_available(conn: &Connection) -> bool {
    let mgr = ModelManager::with_configured_dir(conn);
    mgr.is_installed(MODEL_NAME)
        && mgr.is_installed(TOKENIZER_NAME)
        && mgr.is_installed(crate::ml::clap_audio::MODEL_NAME)
}

/// 把查询串做基础规整：去首尾空白、压缩连续空白。
///
/// 中英文混排时保持原样，交由 tokenizer 处理。
pub fn normalize_query(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_query("  a   b  "), "a b");
        assert_eq!(normalize_query("\t枪声\n"), "枪声");
        assert_eq!(normalize_query("   "), "");
    }

    #[test]
    fn normalize_keeps_cjk_untouched() {
        assert_eq!(normalize_query("沉闷的撞击声"), "沉闷的撞击声");
    }

    #[test]
    fn empty_query_encodes_to_none() {
        let conn = crate::db::open_in_memory().unwrap();
        assert!(encode_if_available(&conn, "   ").unwrap().is_none());
    }

    #[test]
    fn wrong_id_length_is_rejected() {
        let r = embed_ids(Path::new("nope.onnx"), &[1, 2, 3]);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("input_ids 长度错误"));
    }

    #[test]
    fn missing_tokenizer_reports_readable_error() {
        let r = load_tokenizer(Path::new("C:/nope/tokenizer.json"));
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("分词器不存在"));
    }

    #[test]
    fn encode_returns_none_without_model() {
        // 环境变量可能把模型目录指到真有模型的目录，此时这条断言不成立
        if std::env::var("AAM_MODELS_DIR").is_ok() {
            eprintln!("跳过：AAM_MODELS_DIR 已设置，无法断言模型缺失");
            return;
        }
        let conn = crate::db::open_in_memory().unwrap();
        assert!(encode_if_available(&conn, "沉闷的撞击声")
            .unwrap()
            .is_none());
    }

    #[test]
    fn constants_match_documentation() {
        assert_eq!(EMBEDDING_DIM, 512);
        assert_eq!(MAX_TOKENS, 77);
        assert_eq!(PAD_ID, 1);
    }
}
