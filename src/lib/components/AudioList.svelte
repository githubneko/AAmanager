<script lang="ts">
  /**
   * 音频列表（策划书 7.1）。
   *
   * 单击选中、双击播放、Ctrl/Shift 多选；自动标签灰色显示、待确认高亮。
   */
  import { library } from "../stores.svelte";
  import {
    formatDuration,
    formatRelativeTime,
    tagKind,
    sourceLabel,
    displayTag
  } from "../format";
  import type { AudioItem, Tag } from "../types";

  interface Props {
    /** 触发导入（空列表时的引导按钮）。 */
    onimport?: () => void;
    /** 请求打开右键菜单。 */
    oncontextmenu?: (item: AudioItem, x: number, y: number) => void;
  }

  let { onimport, oncontextmenu }: Props = $props();

  /** 当前词汇表映射（规范标签 -> 展示名）。 */
  const labels = $derived(library.tagLabels);

  /** 排序表头定义。 */
  const columns = [
    { key: "name", label: "名称", width: "1fr" },
    { key: "duration", label: "时长", width: "84px" },
    { key: "imported_at", label: "导入", width: "84px" }
  ] as const;

  /** 列表里最多展示几个标签，其余折叠为 +N。 */
  const MAX_VISIBLE_TAGS = 6;

  /** 单击行：选中（Ctrl 追加选择）。 */
  function handleRowClick(event: MouseEvent, item: AudioItem): void {
    library.focus(item.id, event.ctrlKey || event.metaKey);
  }

  /** 双击行：播放。 */
  function handleDoubleClick(item: AudioItem): void {
    void library.play(item.id);
  }

  /** 键盘操作：上下移动、空格播放、回车播放。 */
  function handleKey(event: KeyboardEvent, item: AudioItem, index: number): void {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      const next = library.items[index + 1];
      if (next) library.focus(next.id, event.ctrlKey || event.metaKey);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      const prev = library.items[index - 1];
      if (prev) library.focus(prev.id, event.ctrlKey || event.metaKey);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      void library.play(item.id);
    }
  }

  /** 可见标签 + 折叠数量。 */
  function splitTags(tags: Tag[]): { shown: Tag[]; hidden: number } {
    // 用户标签优先展示，其次按置信度
    const sorted = [...tags].sort((a, b) => {
      const rank = (t: Tag) => (t.source === "user" ? 0 : t.is_confirmed ? 1 : 2);
      return rank(a) - rank(b) || b.confidence - a.confidence;
    });
    return { shown: sorted.slice(0, MAX_VISIBLE_TAGS), hidden: Math.max(0, sorted.length - MAX_VISIBLE_TAGS) };
  }
</script>

<div class="list-wrap">
  <div class="header">
    {#each columns as col (col.key)}
      <button
        class="th"
        class:active={library.query.sort_by === col.key}
        style="width: {col.width}; flex: {col.key === 'name' ? 1 : 'none'}"
        onclick={() => library.setSort(col.key)}
        title="按{col.label}排序"
      >
        {col.label}
        {#if library.query.sort_by === col.key}
          <span class="dir">{library.query.descending ? "↓" : "↑"}</span>
        {/if}
      </button>
    {/each}
    <div class="th tags-col">标签 / 备注</div>
  </div>

  <div class="rows" role="listbox" aria-label="音频列表" aria-multiselectable="true">
    {#each library.items as item, index (item.id)}
      {@const isFocused = library.focusedId === item.id}
      {@const isSelected = library.selected.includes(item.id)}
      {@const isPlaying = library.playback.audio_id === item.id && library.playback.playing}
      {@const tagInfo = splitTags(item.tags)}
      <div
        class="row"
        class:focused={isFocused}
        class:selected={isSelected}
        class:playing={isPlaying}
        class:missing={!item.exists}
        role="option"
        aria-selected={isSelected}
        tabindex={isFocused ? 0 : -1}
        onclick={(e) => handleRowClick(e, item)}
        ondblclick={() => handleDoubleClick(item)}
        oncontextmenu={(e) => {
          // 右键：若目标不在当前选择里，先把它设为唯一选择（与资源管理器一致）
          e.preventDefault();
          if (!library.selected.includes(item.id)) library.focus(item.id);
          library.focusedId = item.id;
          oncontextmenu?.(item, e.clientX, e.clientY);
        }}
        onkeydown={(e) => handleKey(e, item, index)}
        title={item.exists ? item.path : `文件已不存在：${item.path}`}
      >
        <div class="cell name">
          <button
            class="play"
            onclick={(e) => {
              e.stopPropagation();
              void library.play(item.id);
            }}
            aria-label={isPlaying ? "正在播放" : "播放"}
            title={isPlaying ? "正在播放" : "播放"}
          >
            {isPlaying ? "❚❚" : "▶"}
          </button>
          <span class="filename">{item.filename}</span>
          {#if !item.exists}
            <span class="badge danger" title="文件已不在磁盘上">缺失</span>
          {/if}
          {#if item.score !== undefined}
            <span class="badge score" title="匹配度">{Math.round(item.score * 100)}%</span>
          {/if}
        </div>

        <div class="cell num">{formatDuration(item.duration)}</div>
        <div class="cell num dim">{formatRelativeTime(item.created_at)}</div>

        <div class="cell tags">
          {#each tagInfo.shown as tag (tag.id ?? `${tag.source}-${tag.tag}`)}
            {@const shown = displayTag(tag, labels)}
            <span
              class="tag {tagKind(tag)}"
              title="{shown}（规范标签：{tag.tag}） · {sourceLabel(tag.source)} · 置信度 {Math.round(tag.confidence * 100)}%"
            >
              {shown}
              {#if tag.source !== "user" && !tag.is_confirmed}
                <span class="pending-dot" title="待确认">?</span>
              {/if}
            </span>
          {/each}
          {#if tagInfo.hidden > 0}
            <span class="tag more">+{tagInfo.hidden}</span>
          {/if}
          {#if item.note}
            <span class="note" title={item.note}>📝 {item.note}</span>
          {/if}
          {#if item.tags.length === 0 && !item.note}
            <span class="no-tags">
              {library.indexing.pending > 0 ? "等待分析…" : "无标签"}
            </span>
          {/if}
        </div>
      </div>
    {/each}

    {#if library.items.length === 0 && !library.loading}
      <div class="empty">
        {#if library.query.text || library.hasFilters}
          <p class="big">没有匹配的音频</p>
          <p>试试其它关键词，或清空筛选条件。</p>
          <button class="cta" onclick={() => library.clearFilters()}>清空筛选</button>
        {:else}
          <p class="big">音频库还是空的</p>
          <p>把音频文件或文件夹拖到这里，或点击下面的按钮导入。</p>
          <button class="cta" onclick={() => onimport?.()}>选择文件夹导入</button>
        {/if}
      </div>
    {/if}
  </div>

  <div class="footer">
    <span>共 {library.total} 个音频</span>
    {#if library.selected.length > 0}
      <span>· 已选 {library.selected.length}</span>
    {/if}
    {#if library.searchMode !== "keyword"}
      <span class="mode-badge">{library.searchMode === "semantic" ? "语义检索" : "相似检索"}</span>
    {/if}
    {#if library.loading}
      <span class="loading">加载中…</span>
    {/if}
  </div>
</div>

<style>
  .list-wrap {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
    background: var(--bg);
  }

  .header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 0 10px;
    height: 32px;
    border-bottom: 1px solid var(--border);
    background: var(--panel);
  }

  .th {
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    cursor: pointer;
    padding: 4px 2px;
    text-align: left;
    font-family: inherit;
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .th:hover,
  .th.active {
    color: var(--text);
  }

  .th.num,
  .th.tags-col {
    justify-content: flex-start;
  }

  .tags-col {
    flex: 1.4;
    min-width: 0;
  }

  .dir {
    color: var(--accent);
  }

  .rows {
    flex: 1;
    overflow-y: auto;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border-soft);
    cursor: default;
    font-size: 13px;
    min-height: 34px;
  }

  .row:hover {
    background: var(--hover);
  }

  .row.selected {
    background: var(--selected);
  }

  .row.focused {
    box-shadow: inset 2px 0 0 var(--accent);
  }

  .row.playing .filename {
    color: var(--accent);
    font-weight: 600;
  }

  .row.missing .filename {
    text-decoration: line-through;
    opacity: 0.6;
  }

  .row:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .cell {
    flex-shrink: 0;
  }

  .cell.name {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 0;
  }

  .filename {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .play {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--chip-bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-dim);
    font-size: 9px;
    cursor: pointer;
    padding: 0;
  }

  .play:hover {
    color: var(--accent);
    border-color: var(--accent);
  }

  .cell.num {
    width: 84px;
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
    font-size: 12px;
  }

  .cell.num.dim {
    opacity: 0.7;
  }

  .cell.tags {
    flex: 1.4;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 5px;
    overflow: hidden;
  }

  .tag {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 1px 7px;
    border-radius: 999px;
    font-size: 11px;
    white-space: nowrap;
    border: 1px solid transparent;
  }

  /* 用户标签：彩色 */
  .tag.user {
    background: var(--tag-user-bg);
    color: var(--tag-user-fg);
    border-color: var(--tag-user-border);
  }

  /* 已确认的自动标签 */
  .tag.confirmed {
    background: var(--tag-confirmed-bg);
    color: var(--tag-confirmed-fg);
    border-color: var(--tag-confirmed-border);
  }

  /* 自动标签：灰色 */
  .tag.auto {
    background: var(--chip-bg);
    color: var(--text-dim);
    border-color: var(--border);
  }

  /* 低置信度：虚线边框提示待确认 */
  .tag.low {
    background: transparent;
    color: var(--text-dim);
    border: 1px dashed var(--tag-low-border);
    opacity: 0.9;
  }

  .tag.inherited {
    background: var(--tag-inherited-bg);
    color: var(--tag-inherited-fg);
    border-color: var(--tag-inherited-border);
  }

  .tag.more {
    background: transparent;
    color: var(--text-dim);
    border-color: var(--border);
  }

  .pending-dot {
    font-size: 9px;
    color: var(--warn-fg);
  }

  .note {
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
    opacity: 0.85;
  }

  .no-tags {
    font-size: 11px;
    color: var(--text-dim);
    opacity: 0.55;
    font-style: italic;
  }

  .badge {
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 3px;
    flex-shrink: 0;
  }

  .badge.danger {
    background: var(--danger-bg);
    color: var(--danger-fg);
  }

  .badge.score {
    background: var(--info-bg);
    color: var(--tag-user-fg);
  }

  .empty {
    padding: 60px 24px;
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
    line-height: 1.7;
  }

  .empty .big {
    font-size: 16px;
    color: var(--text);
    margin-bottom: 6px;
    font-weight: 600;
  }

  .cta {
    margin-top: 14px;
    background: var(--accent);
    border: none;
    color: var(--accent-contrast);
    font-weight: 600;
    padding: 8px 16px;
    border-radius: 7px;
    cursor: pointer;
    font-size: 13px;
    font-family: inherit;
  }

  .cta:hover {
    filter: brightness(1.1);
  }

  .footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 12px;
    border-top: 1px solid var(--border);
    background: var(--panel);
    font-size: 11px;
    color: var(--text-dim);
  }

  .mode-badge {
    margin-left: auto;
    background: var(--chip-bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 8px;
  }

  .loading {
    margin-left: auto;
  }

  .mode-badge + .loading {
    margin-left: 0;
  }
</style>
