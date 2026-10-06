<script lang="ts">
  /**
   * 标签编辑（策划书 7.2）：自动补全、确认待确认标签、删除标签。
   */
  import { library } from "../stores.svelte";
  import * as api from "../api";
  import { confidenceText, sourceLabel, tagKind, displayTag } from "../format";
  import type { Tag } from "../types";

  interface Props {
    /** 目标音频 id。 */
    audioId: number;
    /** 标签列表。 */
    tags: Tag[];
    /** 批量模式：作用对象为当前选择（用于批量编辑面板）。 */
    bulk?: boolean;
  }

  let { audioId, tags, bulk = false }: Props = $props();

  /** 当前词汇表映射：规范标签 -> 展示名。 */
  const labels = $derived(library.tagLabels);

  /** 输入框内容。 */
  let input = $state("");
  /** 自动补全候选。 */
  let suggestions = $state<string[]>([]);
  /** 高亮的下标。 */
  let highlighted = $state(-1);
  let inputEl = $state<HTMLInputElement | null>(null);

  /** 拉取补全候选（防抖）。 */
  let suggestTimer: ReturnType<typeof setTimeout> | null = null;
  function requestSuggestions(value: string): void {
    if (suggestTimer !== null) clearTimeout(suggestTimer);
    if (!value.trim()) {
      suggestions = [];
      return;
    }
    suggestTimer = setTimeout(async () => {
      try {
        suggestions = await api.suggestTags(value.trim(), 8);
        highlighted = -1;
      } catch {
        suggestions = [];
      }
    }, 120);
  }

  /** 提交标签。 */
  async function submit(value = input): Promise<void> {
    const tag = value.trim();
    if (!tag) return;
    input = "";
    suggestions = [];
    highlighted = -1;
    if (bulk) {
      await library.addTagToSelection(tag);
    } else {
      await library.addTag(audioId, tag);
    }
    inputEl?.focus();
  }

  /** 键盘导航。 */
  function handleKey(event: KeyboardEvent): void {
    if (event.key === "ArrowDown" && suggestions.length > 0) {
      event.preventDefault();
      highlighted = (highlighted + 1) % suggestions.length;
    } else if (event.key === "ArrowUp" && suggestions.length > 0) {
      event.preventDefault();
      highlighted = highlighted <= 0 ? suggestions.length - 1 : highlighted - 1;
    } else if (event.key === "Enter") {
      event.preventDefault();
      void submit(highlighted >= 0 ? suggestions[highlighted] : input);
    } else if (event.key === "Escape") {
      suggestions = [];
      highlighted = -1;
    }
  }

  /** 删除 / 确认。 */
  async function remove(tag: Tag): Promise<void> {
    if (bulk) {
      await api.removeTagMany([...library.selected], tag.tag);
      await library.refresh();
    } else {
      await library.removeTag(audioId, tag.tag);
    }
  }

  async function confirm(tag: Tag): Promise<void> {
    if (!bulk) await library.confirmTag(audioId, tag.tag);
  }
</script>

<div class="tag-editor">
  <div class="tags">
    {#each tags as tag (tag.id ?? `${tag.source}-${tag.tag}`)}
      {@const kind = tagKind(tag)}
      {@const shown = displayTag(tag, labels)}
      <span class="tag {kind}">
        <button
          class="label"
          onclick={() => confirm(tag)}
          title={kind === "user"
            ? "用户标签"
            : `来源 ${sourceLabel(tag.source)} · ${confidenceText(tag)}${tag.is_confirmed ? " · 已确认" : " · 点击确认为准确标签"}`}
        >
          {shown}
          <span class="conf">{confidenceText(tag)}</span>
        </button>
        <button class="x" onclick={() => remove(tag)} title="删除标签" aria-label="删除标签 {shown}">
          ×
        </button>
      </span>
    {/each}
    {#if tags.length === 0}
      <span class="none">还没有标签</span>
    {/if}
  </div>

  <div class="input-wrap">
    <input
      bind:this={inputEl}
      bind:value={input}
      placeholder={bulk ? "为选中的音频添加标签…" : "添加标签，回车确认"}
      oninput={(e) => requestSuggestions((e.currentTarget as HTMLInputElement).value)}
      onkeydown={handleKey}
      aria-label="添加标签"
      aria-autocomplete="list"
    />
    <button class="add" onclick={() => submit()} disabled={!input.trim()}>添加</button>

    {#if suggestions.length > 0}
      <ul class="suggestions" role="listbox">
        {#each suggestions as s, i (s)}
          <li>
            <button
              class="suggestion"
              class:hl={i === highlighted}
              onclick={() => submit(s)}
              role="option"
              aria-selected={i === highlighted}
            >
              {s}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .tag-editor {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: center;
  }

  .tag {
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    border: 1px solid transparent;
    font-size: 12px;
    overflow: hidden;
  }

  .tag .label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    padding: 3px 4px 3px 9px;
    cursor: pointer;
  }

  .tag .conf {
    font-size: 10px;
    opacity: 0.65;
  }

  .tag .x {
    background: none;
    border: none;
    color: inherit;
    opacity: 0.5;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 2px 7px 2px 2px;
  }

  .tag .x:hover {
    opacity: 1;
  }

  .tag.user {
    background: var(--tag-user-bg);
    color: var(--tag-user-fg);
    border-color: var(--tag-user-border);
  }

  .tag.confirmed {
    background: var(--tag-confirmed-bg);
    color: var(--tag-confirmed-fg);
    border-color: var(--tag-confirmed-border);
  }

  .tag.auto {
    background: var(--chip-bg);
    color: var(--text-dim);
    border-color: var(--border);
  }

  .tag.low {
    background: transparent;
    color: var(--text-dim);
    border: 1px dashed var(--tag-low-border);
  }

  .tag.inherited {
    background: var(--tag-inherited-bg);
    color: var(--tag-inherited-fg);
    border-color: var(--tag-inherited-border);
  }

  .none {
    font-size: 12px;
    color: var(--text-dim);
    opacity: 0.7;
  }

  .input-wrap {
    position: relative;
    display: flex;
    gap: 6px;
  }

  .input-wrap input {
    flex: 1;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    color: var(--text);
    padding: 7px 10px;
    font-size: 13px;
    outline: none;
    font-family: inherit;
  }

  .input-wrap input:focus {
    border-color: var(--accent);
  }

  .add {
    background: var(--chip-bg);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 7px;
    padding: 7px 13px;
    font-size: 13px;
    cursor: pointer;
    font-family: inherit;
  }

  .add:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }

  .add:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 60px;
    margin: 4px 0 0;
    padding: 4px;
    list-style: none;
    background: var(--panel-raised);
    border: 1px solid var(--border);
    border-radius: 7px;
    z-index: 20;
    box-shadow: 0 8px 24px var(--shadow-modal);
  }

  .suggestion {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--text);
    padding: 6px 8px;
    font-size: 13px;
    border-radius: 5px;
    cursor: pointer;
    font-family: inherit;
  }

  .suggestion:hover,
  .suggestion.hl {
    background: var(--hover);
    color: var(--accent);
  }
</style>
