<script lang="ts">
  /**
   * 搜索栏：支持「关键词」与「自然语言」两种模式（策划书 2.1 / 2.2）。
   *
   * 关键词模式走 FTS5（含中文 LIKE 兜底），语义模式走 CLAP 向量检索；
   * 模型未安装时后端会自动降级为关键词搜索并通过 `notice` 说明。
   *
   * 数据流（重要）：
   * * `bind:value={text}` 负责「输入框 -> text」；
   * * `$effect` 负责「text -> store.query.text」，store 是**只写**目标；
   * * 另一个 `$effect` 只在 store 被**外部**改动时把值拉回输入框。
   *
   * 之所以不用 `oninput` 手动调 `library.setText(text)`：`bind:value` 与手动
   * 赋值会同时写同一个状态，两者竞争会把输入框内容清空（实测每个按键都会
   * 触发 input 事件但 value 归零）。
   */
  import { untrack } from "svelte";
  import { library } from "../stores.svelte";
  import * as api from "../api";
  import type { SearchResult } from "../types";

  interface Props {
    /** 打开设置页。 */
    onsettings?: () => void;
  }

  let { onsettings }: Props = $props();

  /** 输入框内容（唯一由用户输入写入的状态）。 */
  let text = $state(untrack(() => library.query.text));
  /** 语义模式开关。 */
  let semantic = $state(false);
  /** 语义检索进行中。 */
  let busy = $state(false);
  let inputEl = $state<HTMLInputElement | null>(null);

  /** 上一次由本组件写入 store 的值，用于区分「外部改动」与「自己写入」。 */
  let lastPushed = untrack(() => library.query.text);

  // 输入 -> store（防抖由 store 内部处理）
  $effect(() => {
    const value = text;
    untrack(() => {
      if (value === lastPushed) return;
      lastPushed = value;
      library.setText(value);
    });
  });

  // store -> 输入框：仅当变化不是本组件发起的（例如清空筛选、程序化重置）
  $effect(() => {
    const external = library.query.text;
    untrack(() => {
      if (external === lastPushed) return;
      lastPushed = external;
      text = external;
    });
  });

  /** 执行搜索（语义模式需要显式请求后端）。 */
  async function run(): Promise<void> {
    if (!semantic) {
      library.setText(text);
      return;
    }
    busy = true;
    try {
      const res: SearchResult = await api.semanticSearch(text, library.query.limit);
      // 语义结果直接替换列表，并保留后端给出的说明
      library.items = res.items;
      library.total = res.total;
      library.searchMode = res.mode;
      library.notice = res.notice ?? null;
      library.focusedId = res.items[0]?.id ?? null;
    } catch (e) {
      library.error = (e as Error).message;
    } finally {
      busy = false;
    }
  }

  /** 清空搜索。 */
  function clear(): void {
    text = "";
    semantic = false;
    lastPushed = "";
    library.setQuery({ text: "" }, true);
    inputEl?.focus();
  }

  /** 切换模式后立即重搜。 */
  function toggleMode(): void {
    semantic = !semantic;
    void run();
  }
</script>

<div class="searchbar">
  <div class="field">
    <span class="icon" aria-hidden="true">{semantic ? "✨" : "🔍"}</span>
    <input
      bind:this={inputEl}
      bind:value={text}
      type="text"
      placeholder={semantic
        ? "用自然语言描述你想要的声音，例如：沉闷的撞击声"
        : "搜索文件名、标签、备注…（空格分隔多个关键词）"}
      onkeydown={(e) => {
        if (e.key === "Enter") void run();
        if (e.key === "Escape") clear();
      }}
      aria-label="搜索"
    />
    {#if text}
      <button class="clear" onclick={clear} title="清空" aria-label="清空搜索">×</button>
    {/if}
    {#if busy}
      <span class="spinner" aria-label="搜索中"></span>
    {/if}
  </div>

  <button
    class="mode"
    class:active={semantic}
    onclick={toggleMode}
    title={semantic ? "切换到关键词搜索" : "切换到自然语言搜索"}
  >
    {semantic ? "自然语言" : "关键词"}
  </button>

  {#if !semantic}
    <select
      class="mode-select"
      value={library.query.mode}
      onchange={(e) =>
        library.setQuery(
          { mode: (e.currentTarget as HTMLSelectElement).value as "and" | "or" },
          true
        )}
      title="多关键词的组合方式"
      aria-label="关键词匹配方式"
    >
      <option value="and">全部包含 (AND)</option>
      <option value="or">任一包含 (OR)</option>
    </select>
  {/if}

  <button class="icon-btn" onclick={() => onsettings?.()} title="设置" aria-label="设置">
    ⚙️
  </button>
</div>

<style>
  .searchbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .field {
    position: relative;
    flex: 1;
    display: flex;
    align-items: center;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0 8px;
    transition: border-color 0.15s;
  }

  .field:focus-within {
    border-color: var(--accent);
  }

  .icon {
    font-size: 13px;
    opacity: 0.8;
    margin-right: 6px;
  }

  input {
    flex: 1;
    background: transparent;
    border: none;
    outline: none;
    color: var(--text);
    font-size: 14px;
    padding: 9px 0;
    font-family: inherit;
  }

  input::-webkit-search-cancel-button {
    display: none;
  }

  .clear {
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }

  .clear:hover {
    color: var(--text);
  }

  .spinner {
    width: 13px;
    height: 13px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    margin-left: 4px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .mode,
  .mode-select,
  .icon-btn {
    background: var(--input-bg);
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 8px;
    padding: 9px 12px;
    font-size: 13px;
    cursor: pointer;
    font-family: inherit;
    white-space: nowrap;
  }

  .mode:hover,
  .mode-select:hover,
  .icon-btn:hover {
    color: var(--text);
    border-color: var(--accent);
  }

  .mode.active {
    color: var(--accent-contrast);
    background: var(--accent);
    border-color: var(--accent);
    font-weight: 600;
  }

  .icon-btn {
    padding: 9px 11px;
  }
</style>
