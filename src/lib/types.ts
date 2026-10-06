/**
 * 与 Rust 后端 (`src-tauri/src/models.rs`) 一一对应的类型定义。
 *
 * 命名与序列化格式必须保持一致：后端使用 `#[serde(rename_all = "snake_case")]`
 * 的枚举在这里就是字符串字面量联合类型。
 */

/** 标签来源。 */
export type TagSource = "user" | "yamnet" | "rule" | "inherited" | "clap";

/** 一个标签。 */
export interface Tag {
  id: number | null;
  tag: string;
  source: TagSource;
  /** 0.0 - 1.0 */
  confidence: number;
  is_confirmed: boolean;
}

/** 标签使用频率（标签云）。 */
export interface TagCount {
  tag: string;
  count: number;
  sources: string[];
}

/** 音频文件元数据。 */
export interface AudioFile {
  id: number;
  path: string;
  filename: string;
  duration: number | null;
  sample_rate: number | null;
  channels: number | null;
  format: string | null;
  bitrate: number | null;
  file_size: number | null;
  file_hash: string | null;
  created_at: number;
  updated_at: number;
}

/**
 * 列表条目：元数据 + 标签 + 备注。
 *
 * 后端用 `#[serde(flatten)]` 展开了 `AudioFile`，因此这里直接继承。
 */
export interface AudioItem extends AudioFile {
  tags: Tag[];
  note: string;
  /** 语义 / 相似度检索得分。 */
  score?: number;
  /** 文件当前是否仍存在于磁盘。 */
  exists: boolean;
}

/** 单条用户备注。 */
export interface UserNote {
  id: number;
  audio_id: number;
  note: string;
  created_at: number;
  updated_at: number;
}

/** 排序字段。 */
export type SortBy = "name" | "duration" | "imported_at" | "similarity";

/** 时长区间筛选。 */
export type DurationBucket = "all" | "lt1" | "b1_to_5" | "gt5";

/** 关键词组合方式。 */
export type KeywordMode = "and" | "or";

/** 查询请求。 */
export interface SearchQuery {
  text: string;
  mode: KeywordMode;
  tags: string[];
  sources: string[];
  duration: DurationBucket;
  formats: string[];
  folder: string | null;
  pending_only: boolean;
  sort_by: SortBy;
  descending: boolean;
  limit: number;
  offset: number;
}

/** 搜索响应。 */
export interface SearchResult {
  items: AudioItem[];
  total: number;
  mode: string;
  notice?: string;
}

/** 导入统计。 */
export interface ImportReport {
  imported: number;
  skipped: number;
  failed: number;
  skipped_files: string[];
  errors: string[];
}

/** 导入结果。 */
export interface ImportOutcome extends ImportReport {
  new_ids: number[];
}

/** 后台索引进度。 */
export interface IndexStatus {
  pending: number;
  done: number;
  failed: number;
  running: boolean;
}

/** 播放状态。 */
export interface PlaybackState {
  audio_id: number | null;
  playing: boolean;
  position: number;
  duration: number;
  volume: number;
}

/** 运行模式。 */
export type AppMode = "lite" | "full";

/** 应用设置。 */
export interface Settings {
  mode: AppMode;
  volume: number;
  last_import_dir: string;
  auto_analyze: boolean;
  waveform_points: number;
  /** 界面主题标识。 */
  theme: string;
  /** 自定义模型目录；空串表示使用平台默认目录。 */
  models_dir: string;
}

/** 模型状态。 */
export interface ModelStatus {
  name: string;
  display_name: string;
  purpose: string;
  installed: boolean;
  loaded: boolean;
  path: string;
  expected_size: number;
  actual_size: number;
  /** 实际大小是否与注册表登记一致（用来发现「装的是量化版/旧版本」）。 */
  up_to_date: boolean;
  url: string;
}

/** 模型汇总信息。 */
export interface ModelsInfo {
  models_dir: string;
  /** 平台默认模型目录，供「恢复默认」使用。 */
  default_models_dir: string;
  models: ModelStatus[];
  /** 语义搜索所需模型是否齐备。 */
  semantic_ready: boolean;
  /** 推理内核是否随程序分发（ONNX Runtime 静态链接，恒为 true）。 */
  kernel_bundled: boolean;
  /** 当前已加载的 ONNX 会话数（0 表示还没推理过，属正常）。 */
  inference_sessions: number;
  /** 自动标签（AST 事件分类）是否可用。 */
  tagging_ready: boolean;
  embedded_audio: number;
  /** 正在下载的模型名。 */
  downloading: string[];
  /** 当前模型目录是否可写。 */
  dir_writable: boolean;
}

/** 模型下载信息。 */
export interface ModelDownloadInfo {
  name: string;
  display_name: string;
  purpose: string;
  url: string;
  mirror_url: string | null;
  size_bytes: number;
  /** 是否为语义搜索必需。 */
  required_for_semantic: boolean;
}

/** 库概览统计。 */
export interface LibraryStats {
  total: number;
  tag_count: number;
  formats: [string, number][];
  folders: [string, number][];
}

/** 批量编辑请求。 */
export interface BulkEdit {
  ids: number[];
  add_tags: string[];
  remove_tags: string[];
  note: string | null;
}

/** 批量编辑结果。 */
export interface BulkEditResult {
  affected: number;
  tags_added: number;
  tags_removed: number;
  notes_updated: number;
}

/** 创建一个默认查询。 */
export function defaultQuery(): SearchQuery {
  return {
    text: "",
    mode: "and",
    tags: [],
    sources: [],
    duration: "all",
    formats: [],
    folder: null,
    pending_only: false,
    sort_by: "imported_at",
    descending: true,
    limit: 500,
    offset: 0,
  };
}
