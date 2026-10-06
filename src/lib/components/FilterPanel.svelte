<script lang="ts">
  /**
   * 筛选面板（策划书 7.1）：标签云、来源、时长、格式、文件夹。
   */
  import { library } from "../stores.svelte";

  /** 当前词汇表映射：规范标签 -> 展示名。标签云里的标签都是规则标签。 */
  const labels = $derived(library.tagLabels);

  /** 标签云是否展开（标签多时默认只显示高频的）。 */
  let showAllTags = $state(false);

  /** 可见标签：默认取前 40 个。 */
  const visibleTags = $derived(
    showAllTags ? library.tagCloud : library.tagCloud.slice(0, 40)
  );

  /** 来源选项与展示名。 */
  const sources: { key: string; label: string }[] = [
    { key: "user", label: "用户" },
    { key: "rule", label: "自动规则" },
    { key: "yamnet", label: "YAMNet" },
    { key: "inherited", label: "继承" },
    { key: "clap", label: "CLAP" }
  ];

  /** 时长选项。 */
  const durations: { key: "all" | "lt1" | "b1_to_5" | "gt5"; label: string }[] = [
    { key: "all", label: "全部" },
    { key: "lt1", label: "< 1s" },
    { key: "b1_to_5", label: "1-5s" },
    { key: "gt5", label: "> 5s" }
  ];

  /** 折叠分组状态。 */
  let open = $state({ tags: true, source: true, duration: true, format: true, folder: false });
</script>

<aside class="filters">
  <div class="head">
    <span>筛选</span>
    {#if library.hasFilters}
      <button class="reset" onclick={() => library.clearFilters()}>清空</button>
    {/if}
  </div>

  <!-- 标签云 -->
  <section>
    <button class="group" onclick={() => (open.tags = !open.tags)} aria-expanded={open.tags}>
      <span class="caret" class:closed={!open.tags}>▾</span> 标签
      <span class="count">{library.tagCloud.length}</span>
    </button>
    {#if open.tags}
      <div class="tags">
        {#each visibleTags as t (t.tag)}
          <button
            class="tag-chip"
            class:on={library.query.tags.includes(t.tag)}
            onclick={() => library.toggleTag(t.tag)}
            title="{labels[t.tag] ?? t.tag}（规范标签：{t.tag}） · {t.count} 个音频 · 来源: {t.sources.join(', ')}"
          >
            {labels[t.tag] ?? t.tag}<span class="n">{t.count}</span>
          </button>
        {/each}
        {#if library.tagCloud.length === 0}
          <p class="empty">还没有标签。导入音频后会自动生成，也可以手动添加。</p>
        {/if}
      </div>
      {#if library.tagCloud.length > 40}
        <button class="more" onclick={() => (showAllTags = !showAllTags)}>
          {showAllTags ? "收起" : `展开其余 ${library.tagCloud.length - 40} 个`}
        </button>
      {/if}
    {/if}
  </section>

  <!-- 来源 -->
  <section>
    <button class="group" onclick={() => (open.source = !open.source)} aria-expanded={open.source}>
      <span class="caret" class:closed={!open.source}>▾</span> 来源
    </button>
    {#if open.source}
      <div class="checks">
        {#each sources as s (s.key)}
          <label>
            <input
              type="checkbox"
              checked={library.query.sources.includes(s.key)}
              onchange={() => library.toggleSource(s.key)}
            />
            {s.label}
          </label>
        {/each}
        <label class="pending">
          <input
            type="checkbox"
            checked={library.query.pending_only}
            onchange={(e) =>
              library.setQuery(
                { pending_only: (e.currentTarget as HTMLInputElement).checked },
                true
              )}
          />
          仅看待确认标签
        </label>
      </div>
    {/if}
  </section>

  <!-- 时长 -->
  <section>
    <button
      class="group"
      onclick={() => (open.duration = !open.duration)}
      aria-expanded={open.duration}
    >
      <span class="caret" class:closed={!open.duration}>▾</span> 时长
    </button>
    {#if open.duration}
      <div class="checks">
        {#each durations as d (d.key)}
          <label>
            <input
              type="radio"
              name="duration"
              checked={library.query.duration === d.key}
              onchange={() => library.setQuery({ duration: d.key }, true)}
            />
            {d.label}
          </label>
        {/each}
      </div>
    {/if}
  </section>

  <!-- 格式 -->
  <section>
    <button class="group" onclick={() => (open.format = !open.format)} aria-expanded={open.format}>
      <span class="caret" class:closed={!open.format}>▾</span> 格式
    </button>
    {#if open.format}
      <div class="checks">
        {#each library.stats.formats as f (f[0])}
          <label>
            <input
              type="checkbox"
              checked={library.query.formats.includes(f[0])}
              onchange={() => library.toggleFormat(f[0])}
            />
            {f[0].toUpperCase()}<span class="n">{f[1]}</span>
          </label>
        {/each}
        {#if library.stats.formats.length === 0}
          <p class="empty">尚无音频</p>
        {/if}
      </div>
    {/if}
  </section>

  <!-- 文件夹 -->
  <section>
    <button class="group" onclick={() => (open.folder = !open.folder)} aria-expanded={open.folder}>
      <span class="caret" class:closed={!open.folder}>▾</span> 文件夹
    </button>
    {#if open.folder}
      <div class="checks folders">
        <label>
          <input
            type="radio"
            name="folder"
            checked={library.query.folder === null}
            onchange={() => library.setQuery({ folder: null }, true)}
          />
          全部
        </label>
        {#each library.stats.folders as f (f[0])}
          <label title={f[0]}>
            <input
              type="radio"
              name="folder"
              checked={library.query.folder === f[0]}
              onchange={() => library.setQuery({ folder: f[0] }, true)}
            />
            <span class="path">{f[0].split(/[\\/]/).pop() || f[0]}</span>
            <span class="n">{f[1]}</span>
          </label>
        {/each}
      </div>
    {/if}
  </section>
</aside>

<style>
  .filters {
    width: 232px;
    flex-shrink: 0;
    overflow-y: auto;
    background: var(--panel);
    border-right: 1px solid var(--border);
    padding-bottom: 20px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 12px 14px 8px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-dim);
  }

  .reset {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    font-size: 12px;
    padding: 0;
  }

  section {
    border-top: 1px solid var(--border);
  }

  .group {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    background: none;
    border: none;
    color: var(--text);
    font-size: 13px;
    font-weight: 600;
    padding: 10px 14px;
    cursor: pointer;
    text-align: left;
    font-family: inherit;
  }

  .group:hover {
    background: var(--hover);
  }

  .caret {
    font-size: 10px;
    color: var(--text-dim);
    transition: transform 0.15s;
  }

  .caret.closed {
    transform: rotate(-90deg);
  }

  .count {
    margin-left: auto;
    font-weight: 400;
    color: var(--text-dim);
    font-size: 11px;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    padding: 0 12px 10px;
  }

  .tag-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: var(--chip-bg);
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 999px;
    padding: 3px 9px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
  }

  .tag-chip:hover {
    border-color: var(--accent);
    color: var(--text);
  }

  .tag-chip.on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
  }

  .tag-chip .n {
    opacity: 0.6;
    font-size: 10px;
  }

  .checks {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 14px 12px;
    font-size: 13px;
    color: var(--text-dim);
  }

  .checks label {
    display: flex;
    align-items: center;
    gap: 7px;
    cursor: pointer;
  }

  .checks label:hover {
    color: var(--text);
  }

  .checks input {
    accent-color: var(--accent);
    cursor: pointer;
  }

  .pending {
    border-top: 1px dashed var(--border);
    padding-top: 6px;
    margin-top: 2px;
  }

  .folders .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .checks .n {
    margin-left: auto;
    font-size: 11px;
    opacity: 0.6;
  }

  .empty {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
    opacity: 0.8;
    line-height: 1.5;
  }

  .more {
    background: none;
    border: none;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
    padding: 0 14px 10px;
    font-family: inherit;
  }
</style>
