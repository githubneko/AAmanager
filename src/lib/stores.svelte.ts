/**
 * 全局状态（Svelte 5 runes）。
 *
 * 单例 `library` 贯穿整个应用；所有后端交互都经由 `api.ts`，
 * 状态变更后统一 `refresh()`，保证列表与标签云一致。
 */

import * as api from "./api";
import { applyTheme, normalizeTheme, type ThemeId } from "./theme";
import {
  defaultQuery,
  type AudioItem,
  type ImportOutcome,
  type IndexStatus,
  type LibraryStats,
  type ModelDownloadInfo,
  type ModelsInfo,
  type PlaybackState,
  type SearchQuery,
  type Settings,
  type SortBy,
  type TagCount
} from "./types";

/** 防抖延时（毫秒）——实时搜索但避免每个按键都打后端。 */
const SEARCH_DEBOUNCE_MS = 140;

/** 模型下载进度（与后端 `DownloadProgress` 对应）。 */
export interface DownloadState {
  /** 阶段：connecting / downloading / verifying / done / failed / cancelled。 */
  phase: string;
  downloaded: number;
  total: number | null;
  ratio: number | null;
  bytesPerSec: number;
  source: string;
  error: string | null;
}

export class LibraryStore {
  /** 当前列表条目。 */
  items = $state<AudioItem[]>([]);
  /** 命中总数（可能大于 items.length，因为分页）。 */
  total = $state(0);
  /** 当前查询条件。 */
  query = $state<SearchQuery>(defaultQuery());
  /** 选中的音频 id 集合。 */
  selected = $state<number[]>([]);
  /** 详情面板当前聚焦的音频。 */
  focusedId = $state<number | null>(null);
  /** 播放状态。 */
  playback = $state<PlaybackState>({
    audio_id: null,
    playing: false,
    position: 0,
    duration: 0,
    volume: 0.8
  });
  /** 标签云。 */
  tagCloud = $state<TagCount[]>([]);
  /** 库概览。 */
  stats = $state<LibraryStats>({ total: 0, tag_count: 0, formats: [], folders: [] });
  /** 设置。 */
  settings = $state<Settings | null>(null);
  /** 索引进度。 */
  indexing = $state<IndexStatus>({ pending: 0, done: 0, failed: 0, running: false });
  /** 全局加载标记（首次加载 / 搜索中）。 */
  loading = $state(false);
  /** 搜索模式说明（后端降级时给出提示）。 */
  notice = $state<string | null>(null);
  /** 当前结果来源：关键词 / 语义 / 相似。 */
  searchMode = $state("keyword");
  /** 最近一次导入结果，用于展示提示。 */
  lastImport = $state<ImportOutcome | null>(null);
  /** 错误信息（UI 顶部横幅）。 */
  error = $state<string | null>(null);
  /** 当前主题。 */
  theme = $state<ThemeId>("graphite");
  /** 模型信息（设置面板使用）。 */
  modelsInfo = $state<ModelsInfo | null>(null);
  /** 可下载模型清单。 */
  modelDownloads = $state<ModelDownloadInfo[]>([]);
  /** 各模型的下载进度。 */
  downloadProgress = $state<Record<string, DownloadState>>({});
  /** 标签词汇表清单与当前选择。 */
  vocab = $state<api.VocabList | null>(null);
  /** 已收到的进度事件总数（用于验证事件链路是否真的通了）。 */
  downloadEvents = $state(0);
  /** 下载进度事件的取消订阅函数。 */
  private downloadUnsub: (() => void) | null = null;

  /** 当前聚焦条目。 */
  get focused(): AudioItem | null {
    const id = this.focusedId;
    if (id === null) return null;
    return this.items.find((i) => i.id === id) ?? null;
  }

  /** 选中的条目（批量操作使用）。 */
  get selectedItems(): AudioItem[] {
    const set = new Set(this.selected);
    return this.items.filter((i) => set.has(i.id));
  }

  /** 是否存在筛选条件。 */
  get hasFilters(): boolean {
    const q = this.query;
    return (
      q.tags.length > 0 ||
      q.sources.length > 0 ||
      q.formats.length > 0 ||
      q.duration !== "all" ||
      q.folder !== null ||
      q.pending_only
    );
  }

  // -------------------------------------------------------------------------
  // 初始化与刷新
  // -------------------------------------------------------------------------

  /** 应用启动：并发拉取设置、标签云、统计与列表。 */
  async init(): Promise<void> {
    this.loading = true;
    try {
      const [settings, tags, stats] = await Promise.all([
        api.getSettings(),
        api.listAllTags(),
        api.libraryStats()
      ]);
      this.settings = settings;
      this.theme = normalizeTheme(settings.theme);
      applyTheme(this.theme);
      this.tagCloud = tags;
      this.stats = stats;
      this.playback.volume = settings.volume;
      await this.search(true);
    } catch (e) {
      this.error = (e as Error).message;
    } finally {
      this.loading = false;
    }
    // 模型状态与下载清单失败不应影响主界面
    void this.refreshModels();
    void this.refreshVocab();
    // 启动时就订阅下载进度：否则用户在设置面板外触发的下载收不到进度，
    // 而且设置面板的挂载时机不应该决定事件能否被收到。
    await this.ensureDownloadSubscription();
  }

  /** 确保下载进度事件已订阅（幂等）。 */
  async ensureDownloadSubscription(): Promise<void> {
    if (this.downloadUnsub) return;
    try {
      this.downloadUnsub = await this.subscribeDownloads(() => {
        /* 状态已在 store 内更新，这里无需额外处理 */
      });
    } catch (e) {
      // 非 Tauri 环境（浏览器里跑 vite dev）下没有事件系统
      console.warn("订阅下载进度失败：", e);
    }
  }

  /** 取消订阅（测试或卸载时使用）。 */
  disposeDownloadSubscription(): void {
    this.downloadUnsub?.();
    this.downloadUnsub = null;
  }

  /** 按当前查询取数据；`immediate` 为 true 时跳过防抖。 */
  async search(immediate = false): Promise<void> {
    if (!immediate) {
      this.scheduleSearch();
      return;
    }
    this.loading = true;
    try {
      const res = await api.searchAudio({ ...this.query });
      this.items = res.items;
      this.total = res.total;
      this.searchMode = res.mode;
      this.notice = res.notice ?? null;
      // 选中项若已不在结果中则移除
      const ids = new Set(this.items.map((i) => i.id));
      this.selected = this.selected.filter((id) => ids.has(id));
      if (this.focusedId !== null && !ids.has(this.focusedId)) {
        this.focusedId = this.items[0]?.id ?? null;
      }
      if (this.focusedId === null && this.items.length > 0) {
        this.focusedId = this.items[0].id;
      }
    } catch (e) {
      this.error = (e as Error).message;
    } finally {
      this.loading = false;
    }
  }

  private searchTimer: ReturnType<typeof setTimeout> | null = null;

  /** 防抖搜索（输入即出结果）。 */
  private scheduleSearch(): void {
    if (this.searchTimer !== null) clearTimeout(this.searchTimer);
    this.searchTimer = setTimeout(() => {
      this.searchTimer = null;
      void this.search(true);
    }, SEARCH_DEBOUNCE_MS);
  }

  /** 重新拉取标签云与统计（标签/备注变动后调用）。 */
  async refreshFacets(): Promise<void> {
    try {
      const [tags, stats] = await Promise.all([api.listAllTags(), api.libraryStats()]);
      this.tagCloud = tags;
      this.stats = stats;
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 完整刷新：列表 + 标签云 + 统计。 */
  async refresh(): Promise<void> {
    await this.search(true);
    await this.refreshFacets();
  }

  // -------------------------------------------------------------------------
  // 查询条件变更
  // -------------------------------------------------------------------------

  /** 更新查询条件并触发搜索。 */
  setQuery(patch: Partial<SearchQuery>, immediate = false): void {
    this.query = { ...this.query, ...patch };
    void this.search(immediate);
  }

  /** 设置搜索文本。 */
  setText(text: string): void {
    this.setQuery({ text });
  }

  /** 切换排序（同字段再次点击则反向）。 */
  setSort(sortBy: SortBy): void {
    const descending = this.query.sort_by === sortBy ? !this.query.descending : true;
    this.setQuery({ sort_by: sortBy, descending }, true);
  }

  /** 切换标签筛选。 */
  toggleTag(tag: string): void {
    const tags = this.query.tags.includes(tag)
      ? this.query.tags.filter((t) => t !== tag)
      : [...this.query.tags, tag];
    this.setQuery({ tags }, true);
  }

  /** 切换标签来源筛选。 */
  toggleSource(source: string): void {
    const sources = this.query.sources.includes(source)
      ? this.query.sources.filter((s) => s !== source)
      : [...this.query.sources, source];
    this.setQuery({ sources }, true);
  }

  /** 切换格式筛选。 */
  toggleFormat(format: string): void {
    const formats = this.query.formats.includes(format)
      ? this.query.formats.filter((f) => f !== format)
      : [...this.query.formats, format];
    this.setQuery({ formats }, true);
  }

  /** 清空全部筛选条件（保留搜索文本）。 */
  clearFilters(): void {
    this.setQuery(
      {
        tags: [],
        sources: [],
        formats: [],
        duration: "all",
        folder: null,
        pending_only: false
      },
      true
    );
  }

  // -------------------------------------------------------------------------
  // 选择
  // -------------------------------------------------------------------------

  /** 单击选中（支持 Ctrl/Shift 多选由组件传入）。 */
  focus(id: number, additive = false): void {
    if (additive) {
      this.selected = this.selected.includes(id)
        ? this.selected.filter((s) => s !== id)
        : [...this.selected, id];
      this.focusedId = id;
      return;
    }
    this.selected = [id];
    this.focusedId = id;
  }

  /** 全选 / 取消全选当前结果。 */
  selectAll(): void {
    this.selected =
      this.selected.length === this.items.length ? [] : this.items.map((i) => i.id);
  }

  /** 清空选择。 */
  clearSelection(): void {
    this.selected = [];
  }

  // -------------------------------------------------------------------------
  // 导入
  // -------------------------------------------------------------------------

  /** 导入路径（拖拽）。 */
  async importPaths(paths: string[]): Promise<void> {
    if (paths.length === 0) return;
    this.loading = true;
    try {
      const outcome = await api.importPaths(paths);
      this.lastImport = outcome;
      await this.refresh();
      this.startIndexPolling();
    } catch (e) {
      this.error = (e as Error).message;
    } finally {
      this.loading = false;
    }
  }

  /** 通过系统对话框导入文件夹。 */
  async importFolder(): Promise<void> {
    try {
      const outcome = await api.importFolder();
      if (!outcome) return;
      this.lastImport = outcome;
      await this.refresh();
      this.startIndexPolling();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 通过系统对话框导入文件。 */
  async importFiles(): Promise<void> {
    try {
      const outcome = await api.importFiles();
      if (!outcome) return;
      this.lastImport = outcome;
      await this.refresh();
      this.startIndexPolling();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  // -------------------------------------------------------------------------
  // 索引进度
  // -------------------------------------------------------------------------

  private indexTimer: ReturnType<typeof setInterval> | null = null;

  /** 开始轮询后台分析进度；队列排空后自动停止并刷新列表。 */
  startIndexPolling(): void {
    if (this.indexTimer !== null) return;
    const tick = async () => {
      try {
        this.indexing = await api.indexStatus();
        if (this.indexing.pending === 0 && !this.indexing.running) {
          this.stopIndexPolling();
          // 分析完成后标签/备注已更新，刷新列表与标签云
          await this.refresh();
        }
      } catch {
        this.stopIndexPolling();
      }
    };
    void tick();
    this.indexTimer = setInterval(tick, 1000);
  }

  /** 停止轮询。 */
  stopIndexPolling(): void {
    if (this.indexTimer !== null) {
      clearInterval(this.indexTimer);
      this.indexTimer = null;
    }
  }

  // -------------------------------------------------------------------------
  // 标签 / 备注编辑
  // -------------------------------------------------------------------------

  /** 给当前聚焦（或指定）音频添加标签。 */
  async addTag(audioId: number, tag: string): Promise<void> {
    const trimmed = tag.trim();
    if (!trimmed) return;
    try {
      const tags = await api.addTag(audioId, trimmed);
      this.patchItem(audioId, { tags });
      await this.refreshFacets();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 删除标签。 */
  async removeTag(audioId: number, tag: string): Promise<void> {
    try {
      const tags = await api.removeTag(audioId, tag);
      this.patchItem(audioId, { tags });
      await this.refreshFacets();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 确认自动标签。 */
  async confirmTag(audioId: number, tag: string): Promise<void> {
    try {
      const tags = await api.confirmTag(audioId, tag);
      this.patchItem(audioId, { tags });
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 给选中项批量打标签。 */
  async addTagToSelection(tag: string): Promise<void> {
    const ids = this.selected.length > 0 ? this.selected : this.focusedId ? [this.focusedId] : [];
    if (ids.length === 0 || !tag.trim()) return;
    try {
      await api.addTagMany(ids, tag.trim());
      await this.refresh();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 保存备注。 */
  async saveNote(audioId: number, note: string): Promise<void> {
    try {
      await api.setNote(audioId, note);
      this.patchItem(audioId, { note });
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 批量设置备注。 */
  async saveNoteForSelection(note: string): Promise<void> {
    if (this.selected.length === 0) return;
    try {
      await api.setNoteMany(this.selected, note);
      for (const id of this.selected) this.patchItem(id, { note });
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 就地更新一个条目，避免整表重查。 */
  private patchItem(id: number, patch: Partial<AudioItem>): void {
    this.items = this.items.map((i) => (i.id === id ? { ...i, ...patch } : i));
  }

  // -------------------------------------------------------------------------
  // 删除 / 重新分析
  // -------------------------------------------------------------------------

  /** 从库中移除选中项。 */
  async deleteSelection(): Promise<void> {
    if (this.selected.length === 0) return;
    try {
      await api.deleteAudio([...this.selected]);
      this.selected = [];
      await this.refresh();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 对整个库重算相似度标签继承（用户给一个变体打标签后，让同簇变体跟上）。 */
  async inheritTags(): Promise<void> {
    try {
      const r = await api.inheritTags();
      this.notice =
        r.inherited > 0
          ? `已从相似音频继承 ${r.inherited} 个标签` +
            (r.pruned > 0 ? `，清理 ${r.pruned} 个过期继承` : "") +
            `（比较了 ${r.processed} 个已向量化音频）`
          : `没有可继承的标签（已检查 ${r.processed} 个音频）。` +
            "继承需要：① 已生成音频向量 ② 相似度 ≥ 0.85 的邻居上有你自己打的标签。";
      await this.refresh();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 在文件管理器中定位一个音频（打开所在文件夹并选中文件）。 */
  async reveal(id: number): Promise<void> {
    try {
      await api.revealInFileManager(id);
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 在文件管理器中定位当前选中项（多个时只定位第一个）。 */
  async revealSelection(): Promise<void> {
    const id = this.selected[0] ?? this.focusedId;
    if (id === null) return;
    await this.reveal(id);
  }

  /** 在文件管理器中定位单个音频。 */
  async deleteOne(id: number): Promise<void> {
    try {
      await api.deleteAudio([id]);
      await this.refresh();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 重新分析选中项（或全部）。 */
  async reanalyzeSelection(all = false): Promise<void> {
    try {
      if (all) await api.analyzeAll();
      else if (this.selected.length > 0) await api.reanalyze([...this.selected]);
      else return;
      this.startIndexPolling();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  // -------------------------------------------------------------------------
  // 播放
  // -------------------------------------------------------------------------

  /** 播放一个音频（双击或点击播放按钮）。 */
  async play(id: number): Promise<void> {
    try {
      this.playback = await api.playAudio(id);
      this.focusedId = id;
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 暂停 / 继续（乐观更新：先本地翻转，再等后端确认）。 */
  async togglePlayPause(): Promise<void> {
    const state = this.playback;
    if (state.audio_id === null) {
      const first = this.focusedId ?? this.items[0]?.id;
      if (first !== undefined) await this.play(first);
      return;
    }

    const wasPlaying = state.playing;
    // 立刻翻转按钮与播放头，不等 IPC 往返
    this.playback = { ...state, playing: !wasPlaying };

    try {
      this.playback = wasPlaying ? await api.pauseAudio() : await api.resumeAudio();
    } catch (e) {
      // 失败则回滚
      this.playback = { ...this.playback, playing: wasPlaying };
      this.error = (e as Error).message;
    }
  }

  /** 停止。 */
  async stop(): Promise<void> {
    // 乐观清空，避免按钮短暂残留播放态
    this.playback = { ...this.playback, playing: false, position: 0 };
    try {
      this.playback = await api.stopAudio();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 跳转（乐观移动播放头）。 */
  async seek(seconds: number): Promise<void> {
    this.playback = { ...this.playback, position: seconds };
    try {
      this.playback = await api.seekAudio(seconds);
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 调整音量并持久化到设置。 */
  async changeVolume(volume: number): Promise<void> {
    try {
      this.playback = await api.setVolume(volume);
      if (this.settings) {
        // 展开即为普通对象，可直接交给 IPC 序列化
        const next: Settings = { ...this.settings, volume };
        this.settings = await api.saveSettings(next);
      }
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 拉取一次播放进度（前端定时调用）。 */
  async syncPlayback(): Promise<void> {
    try {
      this.playback = await api.playbackState();
    } catch {
      /* 播放状态查询失败不打扰用户 */
    }
  }

  /** 播放列表内的下一个音频。 */
  async playNext(offset = 1): Promise<void> {
    const idx = this.items.findIndex((i) => i.id === this.focusedId);
    if (idx < 0) return;
    const next = idx + offset;
    if (next < 0 || next >= this.items.length) return;
    await this.play(this.items[next].id);
  }

  // -------------------------------------------------------------------------
  // 设置
  // -------------------------------------------------------------------------

  /** 保存设置。 */
  async updateSettings(patch: Partial<Settings>): Promise<void> {
    if (!this.settings) return;
    const next = { ...this.settings, ...patch };
    try {
      this.settings = await api.saveSettings(next);
      // 主题是本地表现，保存成功后立即生效（无需重载）
      this.theme = normalizeTheme(this.settings.theme);
      applyTheme(this.theme);
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 切换主题并持久化。 */
  async setTheme(id: string): Promise<void> {
    const theme = normalizeTheme(id);
    // 立即应用，界面无延迟；持久化失败也不会回滚视觉
    this.theme = theme;
    applyTheme(theme);
    await this.updateSettings({ theme });
  }

  // -------------------------------------------------------------------------
  // 模型管理
  // -------------------------------------------------------------------------

  /** 刷新模型状态与可下载清单。 */
  async refreshModels(): Promise<void> {
    try {
      const [info, list] = await Promise.all([api.modelsInfo(), api.modelDownloadList()]);
      this.modelsInfo = info;
      this.modelDownloads = list;
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 订阅后端下载进度事件。返回取消订阅函数。 */
  async subscribeDownloads(
    onEvent: (name: string, state: DownloadState) => void
  ): Promise<() => void> {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<{
      name: string;
      phase: string;
      downloaded: number;
      total: number | null;
      ratio: number | null;
      bytes_per_sec: number;
      source: string;
      error: string | null;
    }>("model-download", (event) => {
      const p = event.payload;
      this.downloadEvents += 1;
      const state: DownloadState = {
        phase: p.phase,
        downloaded: p.downloaded,
        total: p.total,
        ratio: p.ratio,
        bytesPerSec: p.bytes_per_sec,
        source: p.source,
        error: p.error
      };
      this.downloadProgress = { ...this.downloadProgress, [p.name]: state };
      onEvent(p.name, state);
    });
    return unlisten;
  }

  /**
   * 一键下载模型。
   *
   * 下载期间后端持续发出进度事件；本方法在完成后刷新模型状态。
   */
  async downloadModel(name: string): Promise<boolean> {
    this.downloadProgress = {
      ...this.downloadProgress,
      [name]: {
        phase: "connecting",
        downloaded: 0,
        total: null,
        ratio: null,
        bytesPerSec: 0,
        source: "",
        error: null
      }
    };
    try {
      await api.downloadModel(name);
      await this.refreshModels();
      // 成功后保留 done 状态一小段时间，让用户看到完成
      setTimeout(() => {
        const { [name]: _drop, ...rest } = this.downloadProgress;
        this.downloadProgress = rest;
      }, 2500);
      return true;
    } catch (e) {
      const msg = (e as Error).message;
      this.downloadProgress = {
        ...this.downloadProgress,
        [name]: {
          ...(this.downloadProgress[name] ?? {
            downloaded: 0,
            total: null,
            ratio: null,
            bytesPerSec: 0,
            source: ""
          }),
          phase: "failed",
          error: msg
        }
      };
      return false;
    }
  }

  /** 取消下载。 */
  async cancelDownload(name: string): Promise<void> {
    try {
      await api.cancelDownload(name);
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 用系统对话框更改模型目录。返回新目录，取消时为 null。 */
  async pickModelsDir(): Promise<string | null> {
    try {
      const dir = await api.pickModelsDir();
      if (dir) {
        await this.refreshModels();
        if (this.settings) this.settings = { ...this.settings, models_dir: dir };
      }
      return dir;
    } catch (e) {
      this.error = (e as Error).message;
      return null;
    }
  }

  /** 恢复默认模型目录。 */
  async resetModelsDir(): Promise<void> {
    try {
      const dir = await api.resetModelsDir();
      if (this.settings) this.settings = { ...this.settings, models_dir: "" };
      await this.refreshModels();
      void dir;
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 删除已下载的模型。 */
  async deleteModel(name: string): Promise<void> {
    try {
      await api.deleteModel(name);
      await this.refreshModels();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 卸载模型（释放内存）。 */
  async unloadModel(name: string): Promise<void> {
    try {
      await api.unloadModel(name);
      await this.refreshModels();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 在文件管理器中打开模型目录。 */
  async revealModelsDir(): Promise<void> {
    try {
      await api.revealModelsDir();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  // -------------------------------------------------------------------------
  // 标签词汇表
  // -------------------------------------------------------------------------

  /** 当前词汇表的映射表：规范标签 -> 展示名。 */
  get tagLabels(): Record<string, string> {
    const list = this.vocab;
    if (!list) return {};
    const current = list.vocabularies.find((v) => v.id === list.current);
    if (!current) return {};
    return Object.fromEntries(current.table);
  }

  /** 当前词汇表的展示名。 */
  get vocabLabel(): string {
    return this.vocab?.vocabularies.find((v) => v.id === this.vocab?.current)?.label ?? "";
  }

  /** 刷新词汇表清单。 */
  async refreshVocab(): Promise<void> {
    try {
      this.vocab = await api.listTagVocabularies();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 切换词汇表（后端会重写自动备注）。 */
  async setVocab(id: string): Promise<void> {
    try {
      const updated = await api.setTagVocabulary(id);
      await this.refreshVocab();
      // 备注文案变了，刷新列表与统计
      await this.refresh();
      if (updated > 0) {
        this.notice = `已切换为「${this.vocabLabel}」，并更新了 ${updated} 条自动备注`;
      }
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 设置 / 清除自定义词汇改写。 */
  async setVocabOverride(canonical: string, label: string | null): Promise<void> {
    try {
      await api.setTagOverride(canonical, label);
      await this.refreshVocab();
      await this.refresh();
    } catch (e) {
      this.error = (e as Error).message;
    }
  }

  /** 清空错误提示。 */
  clearError(): void {
    this.error = null;
  }

  /** 清空导入提示。 */
  clearImportNotice(): void {
    this.lastImport = null;
  }

  /** 清空搜索说明。 */
  clearNotice(): void {
    this.notice = null;
  }
}

/** 全局单例。 */
export const library = new LibraryStore();
