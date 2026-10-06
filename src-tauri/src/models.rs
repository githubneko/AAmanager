//! 领域模型定义（前后端共享的数据结构）。

use serde::{Deserialize, Serialize};

/// 标签来源，对应 `tags.source` 列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagSource {
    /// 用户手动添加。
    User,
    /// YAMNet 模型推理得到。
    Yamnet,
    /// 规则引擎根据声学特征生成。
    Rule,
    /// 由相似音频继承而来。
    Inherited,
    /// CLAP 语义模型生成。
    Clap,
}

impl TagSource {
    /// 转换为数据库存储的字符串。
    pub fn as_db(&self) -> &'static str {
        match self {
            TagSource::User => "user",
            TagSource::Yamnet => "yamnet",
            TagSource::Rule => "rule",
            TagSource::Inherited => "inherited",
            TagSource::Clap => "clap",
        }
    }

    /// 从数据库字符串解析。未知取值一律归为 [`TagSource::Rule`]。
    ///
    /// 方法名不用 `from_str`，避免与标准库的 [`std::str::FromStr`] 混淆。
    pub fn from_db(s: &str) -> Self {
        match s {
            "user" => TagSource::User,
            "yamnet" => TagSource::Yamnet,
            "inherited" => TagSource::Inherited,
            "clap" => TagSource::Clap,
            _ => TagSource::Rule,
        }
    }

    /// 是否属于「自动生成」（前端以灰色显示）。
    pub fn is_auto(&self) -> bool {
        !matches!(self, TagSource::User)
    }
}

/// 一条音频文件的完整记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFile {
    /// 主键。
    pub id: i64,
    /// 磁盘绝对路径。
    pub path: String,
    /// 文件名。
    pub filename: String,
    /// 时长（秒）。
    pub duration: Option<f64>,
    /// 采样率（Hz）。
    pub sample_rate: Option<i64>,
    /// 声道数。
    pub channels: Option<i64>,
    /// 容器 / 编码格式，例如 `wav`、`mp3`。
    pub format: Option<String>,
    /// 码率（bit/s）。
    pub bitrate: Option<i64>,
    /// 文件字节数。
    pub file_size: Option<i64>,
    /// 内容哈希，用于去重。
    pub file_hash: Option<String>,
    /// 导入时间（Unix 秒）。
    pub created_at: i64,
    /// 最近更新时间（Unix 秒）。
    pub updated_at: i64,
}

/// 列表 / 详情场景下返回给前端的音频条目（含标签与备注）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioItem {
    /// 基础元数据。
    #[serde(flatten)]
    pub audio: AudioFile,
    /// 关联标签。
    pub tags: Vec<Tag>,
    /// 用户备注全文（多条备注以换行拼接）。
    pub note: String,
    /// 语义检索得分（仅语义搜索时出现）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// 文件当前是否仍存在于磁盘。
    pub exists: bool,
}

/// 一个标签。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    /// 主键；未落库时为 `None`。
    pub id: Option<i64>,
    /// 标签文本。
    pub tag: String,
    /// 来源。
    pub source: TagSource,
    /// 置信度 0.0 - 1.0；用户标签恒为 1.0。
    pub confidence: f32,
    /// 用户是否已确认（自动标签确认后前端转为彩色显示）。
    pub is_confirmed: bool,
}

impl Tag {
    /// 构造一个标签。
    pub fn new(tag: impl Into<String>, source: TagSource, confidence: f32) -> Self {
        Self {
            id: None,
            tag: tag.into(),
            source,
            confidence: confidence.clamp(0.0, 1.0),
            is_confirmed: source == TagSource::User,
        }
    }
}

/// 标签使用频率统计（用于标签云）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCount {
    /// 标签文本。
    pub tag: String,
    /// 使用次数。
    pub count: i64,
    /// 该标签出现过的来源种类。
    pub sources: Vec<String>,
}

/// 单条用户备注。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserNote {
    /// 主键。
    pub id: i64,
    /// 所属音频 id。
    pub audio_id: i64,
    /// 备注正文。
    pub note: String,
    /// 创建时间（Unix 秒）。
    pub created_at: i64,
    /// 更新时间（Unix 秒）。
    pub updated_at: i64,
}

/// 排序字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// 按文件名。
    Name,
    /// 按时长。
    Duration,
    /// 按导入时间。
    #[default]
    ImportedAt,
    /// 按相似度（仅语义搜索有意义）。
    Similarity,
}

/// 时长区间筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationBucket {
    /// 不限制。
    #[default]
    All,
    /// < 1 秒。
    Lt1,
    /// 1 - 5 秒。
    B1To5,
    /// > 5 秒。
    Gt5,
}

/// 查询请求：关键词搜索 + 筛选 + 排序 + 分页。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchQuery {
    /// 原始查询串（关键词或自然语言）。
    pub text: String,
    /// 词汇表展开出的额外检索词（由命令层填充，前端不必传）。
    ///
    /// 这些词与 `text` 一起按**并集**检索，并按命中关键词数量排序：
    /// 用户搜「爆炸」（游戏词汇表下「沉闷」的展示名）时，
    /// 这里会带上规范标签「沉闷」，于是能命中真正带该标签的音频。
    #[serde(default)]
    pub extra_terms: Vec<String>,
    /// 多关键词组合方式：`and` / `or`。
    pub mode: KeywordMode,
    /// 必须包含的标签（AND 语义）。
    pub tags: Vec<String>,
    /// 筛选：标签来源，空表示不限。
    pub sources: Vec<String>,
    /// 筛选：时长区间。
    pub duration: DurationBucket,
    /// 筛选：文件格式（如 `wav`），空表示不限。
    pub formats: Vec<String>,
    /// 筛选：所在文件夹（路径前缀）。
    pub folder: Option<String>,
    /// 只看「待确认」的自动标签。
    pub pending_only: bool,
    /// 排序字段。
    pub sort_by: SortBy,
    /// 是否降序。
    pub descending: bool,
    /// 返回条数上限。
    pub limit: u32,
    /// 偏移量。
    pub offset: u32,
}

/// 关键词组合方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeywordMode {
    /// 全部关键词都要命中。
    #[default]
    And,
    /// 任一关键词命中即可。
    Or,
}

/// 搜索响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// 命中条目。
    pub items: Vec<AudioItem>,
    /// 命中总数（用于分页）。
    pub total: u32,
    /// 本次检索实际使用的模式，`keyword` 或 `semantic`。
    pub mode: String,
    /// 语义检索不可用时的说明（例如模型未下载）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// 导入结果统计。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportReport {
    /// 新导入的文件数。
    pub imported: u32,
    /// 因路径或哈希重复而跳过的文件数。
    pub skipped: u32,
    /// 解析失败的文件数。
    pub failed: u32,
    /// 跳过的重名清单（便于前端提示）。
    pub skipped_files: Vec<String>,
    /// 失败清单及原因。
    pub errors: Vec<String>,
}

/// 声学特征（规则引擎与后续 ML 模块的输入）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct AcousticFeatures {
    /// 时长（秒）。
    pub duration: f32,
    /// 均方根能量。
    pub rms: f32,
    /// 过零率。
    pub zcr: f32,
    /// 频谱质心（Hz）。
    pub spectral_centroid: f32,
    /// 频谱滚降点（Hz，85% 能量）。
    pub spectral_rolloff: f32,
    /// 频谱平坦度（0 = 纯音，1 = 噪声）。
    pub spectral_flatness: f32,
    /// 起振时间（秒）。
    pub attack_time: f32,
    /// 衰减时间（秒）。
    pub decay_time: f32,
}

/// 运行模式（策划书 2.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    /// 轻量模式：仅关键词 + 自动标签。
    #[default]
    Lite,
    /// 完整模式：追加语义搜索。
    Full,
}

/// 后台索引任务的进度快照。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexStatus {
    /// 队列中剩余任务数。
    pub pending: u32,
    /// 已完成任务数。
    pub done: u32,
    /// 失败任务数。
    pub failed: u32,
    /// 是否正在处理。
    pub running: bool,
}

/// 播放状态（直接复用播放器模块的定义，避免重复结构）。
pub type PlaybackStateDto = crate::audio::player::PlaybackState;
