<script lang="ts">
  /**
   * 批量编辑工具栏（策划书 2.1「支持批量编辑」）。
   *
   * 当列表中选中多个音频时出现，可批量打标签、写备注、重新分析、移出库。
   */
  import TagEditor from "./TagEditor.svelte";
  import NoteEditor from "./NoteEditor.svelte";
  import { library } from "../stores.svelte";
  import type { Tag } from "../types";

  interface Props {
    /** 关闭批量面板。 */
    onclose?: () => void;
  }

  let { onclose }: Props = $props();

  /** 选中的条目。 */
  const items = $derived(library.selectedItems);
  /** 选中项共有的标签（去重）。 */
  const commonTags = $derived.by((): Tag[] => {
    if (items.length === 0) return [];
    // 取所有选中项都拥有的标签（按 tag 文本判定）
    const counts = new Map<string, { tag: Tag; n: number }>();
    for (const item of items) {
      for (const t of item.tags) {
        const cur = counts.get(t.tag);
        if (cur) cur.n += 1;
        else counts.set(t.tag, { tag: t, n: 1 });
      }
    }
    return [...counts.values()].filter((c) => c.n === items.length).map((c) => c.tag);
  });
  /** 选中项备注一致时的公共备注，否则为空串。 */
  const commonNote = $derived.by(() => {
    if (items.length === 0) return "";
    const first = items[0].note;
    return items.every((i) => i.note === first) ? first : "";
  });

  /** 确认移除标签。 */
  async function handleRemove(tag: Tag): Promise<void> {
    const { removeTagMany } = await import("../api");
    await removeTagMany([...library.selected], tag.tag);
    await library.refresh();
  }
</script>

<div class="bulk">
  <div class="head">
    <strong>批量编辑</strong>
    <span class="count">已选中 {items.length} 个音频</span>
    <button class="link" onclick={() => library.clearSelection()}>取消选择</button>
    <button class="link danger" onclick={() => library.deleteSelection()}>
      从库中移除
    </button>
    <button class="link" onclick={() => library.reanalyzeSelection()}>重新分析</button>
    {#if onclose}
      <button class="close" onclick={onclose} aria-label="关闭批量面板">×</button>
    {/if}
  </div>

  <div class="body">
    <div class="field">
      <label for="bulk-tags">标签（对选中的全部音频生效）</label>
      {#if commonTags.length > 0}
        <div class="shared">
          <span class="shared-label">共有标签：</span>
          {#each commonTags as tag (tag.tag)}
            <span class="tag-chip">
              {tag.tag}
              <button onclick={() => handleRemove(tag)} title="从所有选中项移除">×</button>
            </span>
          {/each}
        </div>
      {/if}
      <TagEditor audioId={items[0]?.id ?? 0} tags={[]} bulk />
    </div>

    <div class="field">
      <label for="bulk-note">备注</label>
      {#if commonNote}
        <NoteEditor
          audioId={items[0]?.id ?? 0}
          note={commonNote}
          saveLabel="应用到全部"
          onsave={(n) => library.saveNoteForSelection(n)}
        />
      {:else}
        <p class="mixed">选中的音频备注各不相同。写入内容会覆盖全部选中项：</p>
        <NoteEditor
          audioId={0}
          note=""
          saveLabel="应用到全部"
          onsave={(n) => library.saveNoteForSelection(n)}
        />
      {/if}
    </div>
  </div>
</div>

<style>
  .bulk {
    border-top: 1px solid var(--border);
    background: var(--panel-raised);
    flex-shrink: 0;
    max-height: 42vh;
    overflow-y: auto;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }

  .count {
    color: var(--text-dim);
    font-size: 12px;
  }

  .link {
    background: none;
    border: none;
    color: var(--accent);
    font-size: 12px;
    cursor: pointer;
    padding: 0;
    font-family: inherit;
  }

  .link.danger {
    color: var(--danger-fg);
  }

  .link:hover {
    text-decoration: underline;
  }

  .close {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 18px;
    cursor: pointer;
    line-height: 1;
  }

  .body {
    display: flex;
    gap: 18px;
    padding: 12px 14px 14px;
    flex-wrap: wrap;
  }

  .field {
    flex: 1;
    min-width: 280px;
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  .field > label {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
  }

  .shared {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    font-size: 12px;
  }

  .shared-label {
    color: var(--text-dim);
    font-size: 11px;
  }

  .tag-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: var(--chip-bg);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 2px 4px 2px 9px;
    font-size: 11px;
    color: var(--text-dim);
  }

  .tag-chip button {
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font-size: 13px;
    line-height: 1;
    padding: 0 4px;
    opacity: 0.6;
  }

  .tag-chip button:hover {
    opacity: 1;
    color: var(--danger-fg);
  }

  .mixed {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.6;
  }
</style>
