/**
 * Tauri 命令封装。所有后端调用的唯一入口，便于统一处理错误。
 *
 * 命令名与 `src-tauri/src/lib.rs` 里的 `generate_handler!` 列表一一对应。
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AudioItem,
  BulkEdit,
  BulkEditResult,
  ImportOutcome,
  IndexStatus,
  LibraryStats,
  ModelDownloadInfo,
  ModelsInfo,
  PlaybackState,
  SearchQuery,
  SearchResult,
  Settings,
  Tag,
  TagCount,
  UserNote
} from "./types";

/** 把后端返回的错误统一转成 Error，便于 UI 展示可读信息。 */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    // 后端错误已序列化为字符串
    throw new Error(typeof e === "string" ? e : String(e));
  }
}

// ---------------------------------------------------------------------------
// 导入 / 列表
// ---------------------------------------------------------------------------

/** 导入文件或文件夹路径（拖拽入口）。 */
export const importPaths = (paths: string[]) =>
  call<ImportOutcome>("import_paths", { paths });

/** 弹出目录选择框并导入。返回 null 表示用户取消。 */
export const importFolder = () => call<ImportOutcome | null>("import_folder");

/** 弹出文件选择框并导入。返回 null 表示用户取消。 */
export const importFiles = () => call<ImportOutcome | null>("import_files");

/** 取波形峰值（0-255）。 */
export const getWaveform = (audioId: number) =>
  call<number[]>("get_waveform", { audioId });

/** 取单个音频（含标签与备注）。 */
export const getAudio = (audioId: number) => call<AudioItem>("get_audio", { audioId });

/** 列出音频。 */
export const listAudio = (limit = 1000) => call<AudioItem[]>("list_audio", { limit });

/** 从库中移除（不删除磁盘文件）。 */
export const deleteAudio = (ids: number[]) => call<number>("delete_audio", { ids });

/** 在系统文件管理器中定位该音频（打开所在文件夹并选中文件）。 */
export const revealInFileManager = (audioId: number) =>
  call<string>("reveal_in_file_manager", { audioId });

/** 对整个库重算相似度标签继承。 */
export const inheritTags = () =>
  call<{ processed: number; inherited: number; pruned: number }>("inherit_tags");

/** 重新分析指定音频。 */
export const reanalyze = (ids: number[]) => call<number>("reanalyze", { ids });

/** 分析所有尚无自动标签的音频。 */
export const analyzeAll = () => call<number>("analyze_all");

/** 后台索引进度。 */
export const indexStatus = () => call<IndexStatus>("index_status");

// ---------------------------------------------------------------------------
// 播放
// ---------------------------------------------------------------------------

export const playAudio = (audioId: number) =>
  call<PlaybackState>("play_audio", { audioId });

export const pauseAudio = () => call<PlaybackState>("pause_audio");

export const resumeAudio = () => call<PlaybackState>("resume_audio");

export const stopAudio = () => call<PlaybackState>("stop_audio");

export const seekAudio = (seconds: number) =>
  call<PlaybackState>("seek_audio", { seconds });

export const setVolume = (volume: number) =>
  call<PlaybackState>("set_volume", { volume });

export const playbackState = () => call<PlaybackState>("playback_state");

// ---------------------------------------------------------------------------
// 标签
// ---------------------------------------------------------------------------

export const listTags = (audioId: number) => call<Tag[]>("list_tags", { audioId });

export const listAllTags = (limit = 300) => call<TagCount[]>("list_all_tags", { limit });

export const suggestTags = (prefix: string, limit = 20) =>
  call<string[]>("suggest_tags", { prefix, limit });

export const addTag = (audioId: number, tag: string) =>
  call<Tag[]>("add_tag", { audioId, tag });

export const addTagMany = (ids: number[], tag: string) =>
  call<number>("add_tag_many", { ids, tag });

export const removeTag = (audioId: number, tag: string) =>
  call<Tag[]>("remove_tag", { audioId, tag });

export const removeTagMany = (ids: number[], tag: string) =>
  call<number>("remove_tag_many", { ids, tag });

export const confirmTag = (audioId: number, tag: string) =>
  call<Tag[]>("confirm_tag", { audioId, tag });

export const renameTag = (from: string, to: string) =>
  call<number>("rename_tag", { from, to });

// ---------------------------------------------------------------------------
// 备注
// ---------------------------------------------------------------------------

export const listNotes = (audioId: number) => call<UserNote[]>("list_notes", { audioId });

export const setNote = (audioId: number, note: string) =>
  call<string>("set_note", { audioId, note });

export const setNoteMany = (ids: number[], note: string) =>
  call<number>("set_note_many", { ids, note });

export const appendNote = (audioId: number, note: string) =>
  call<UserNote[]>("append_note", { audioId, note });

export const deleteNote = (noteId: number) => call<void>("delete_note", { noteId });

/** 批量编辑标签与备注。 */
export const bulkEdit = (edit: BulkEdit) => call<BulkEditResult>("bulk_edit", { edit });

/** 库概览统计。 */
export const libraryStats = () => call<LibraryStats>("library_stats");

// ---------------------------------------------------------------------------
// 搜索
// ---------------------------------------------------------------------------

/** 关键词 + 筛选检索。 */
export const searchAudio = (query: SearchQuery) =>
  call<SearchResult>("search_audio", { query });

/** 自然语言检索（模型缺失时后端自动降级为关键词搜索）。 */
export const semanticSearch = (text: string, limit = 50) =>
  call<SearchResult>("semantic_search", { text, limit });

/** 相似音频检索。 */
export const similarAudio = (audioId: number, limit = 20) =>
  call<SearchResult>("similar_audio", { audioId, limit });

// ---------------------------------------------------------------------------
// 设置 / 模型
// ---------------------------------------------------------------------------

export const getSettings = () => call<Settings>("get_settings");

export const saveSettings = (settings: Settings) =>
  call<Settings>("save_settings", { settings });

export const modelsInfo = () => call<ModelsInfo>("models_info");

export const modelDownloadList = () =>
  call<ModelDownloadInfo[]>("model_download_list");

/** 一键下载模型；进度由后端 `model-download` 事件上报。 */
export const downloadModel = (name: string) =>
  call<{ name: string; path: string; bytes: number; elapsed_ms: number }>("download_model", {
    name
  });

/** 取消正在进行的下载。 */
export const cancelDownload = (name: string) => call<boolean>("cancel_download", { name });

/** 用系统对话框选择模型目录（后端已校验可写并写入设置）。 */
export const pickModelsDir = () => call<string | null>("pick_models_dir");

/** 恢复默认模型目录。 */
export const resetModelsDir = () => call<string>("reset_models_dir");

/** 列出内置主题标识（后端为唯一来源）。 */
export const listThemes = () => call<string[]>("list_themes");

// ---------------------------------------------------------------------------
// 标签词汇表
// ---------------------------------------------------------------------------

/** 一个词汇表及其对照表。 */
export interface VocabInfo {
  id: string;
  label: string;
  audience: string;
  hint: string;
  /** 规范标签 -> 该词汇表下的说法。 */
  table: [string, string][];
}

/** 词汇表清单与当前选择。 */
export interface VocabList {
  current: string;
  overrides: Record<string, string>;
  vocabularies: VocabInfo[];
}

/** 列出全部标签词汇表。 */
export const listTagVocabularies = () => call<VocabList>("list_tag_vocabularies");

/** 切换词汇表；返回被重写自动备注的条数。 */
export const setTagVocabulary = (vocab: string) =>
  call<number>("set_tag_vocabulary", { vocab });

/** 设置 / 清除一个自定义词汇改写。`label` 为 null 表示恢复默认。 */
export const setTagOverride = (canonical: string, label: string | null) =>
  call<number>("set_tag_override", { canonical, label });

export const unloadModel = (name?: string) => call<void>("unload_model", { name: name ?? null });

export const deleteModel = (name: string) => call<void>("delete_model", { name });

export const revealModelsDir = () => call<string>("reveal_models_dir");

// ---------------------------------------------------------------------------
// 仅开发期：前端把渲染自述回传后端落盘，供自动化验证断言
// ---------------------------------------------------------------------------

/** 渲染自述载荷。 */
export interface UiReport {
  mounted: boolean;
  title: string;
  rows: number;
  empty_hint: boolean;
  has_search: boolean;
  has_filters: boolean;
  has_toolbar: boolean;
  init_ok: boolean;
  error: string | null;
  total: number;
  search_mode: string;
  app_mode: string;
}

/**
 * 上报渲染结果。仅 debug 构建注册了该命令；
 * 生产构建调用会失败，因此调用方必须自行用 `import.meta.env.DEV` 包裹。
 */
export const debugReportUi = (report: UiReport) =>
  call<string>("debug_report_ui", { report });

/**
 * 读取 `AAM_AUTO_IMPORT` 环境变量指定的目录（未设置时返回 `null`）。
 * 仅 debug 构建注册，供端到端验证脚本驱动导入。
 */
export const debugAutoImportDir = () => call<string | null>("debug_auto_import_dir");

/**
 * 落盘当前页面状态快照（仅 debug 构建）。开发期由页面定时调用，
 * 外部脚本读取 `page-state.json` 即可看到输入框真实内容与焦点。
 */
export const debugWritePageState = (state: Record<string, unknown>) =>
  call<string>("debug_write_page_state", { state });

/** `AAM_PROBE_FOCUS=1` 时返回 true，用于自动化验证中单独测试键盘输入。 */
export const debugShouldFocusSearch = () => call<boolean>("debug_should_focus_search");

/**
 * 取走外部脚本投递的命令（仅 debug 构建）。
 *
 * 本机 WebView2 不允许开远程调试端口，因此用文件队列代替 CDP：
 * 脚本写 `cmd-queue.json`，页面轮询执行后把结果写回 `cmd-result.json`。
 */
export const debugTakeCommands = () =>
  call<Record<string, unknown>[]>("debug_take_commands");

/** 写回命令执行结果（仅 debug 构建）。 */
export const debugWriteCommandResult = (result: Record<string, unknown>) =>
  call<string>("debug_write_command_result", { result });
