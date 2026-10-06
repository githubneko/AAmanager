<script lang="ts">
  /**
   * 右键菜单（策划书 7.2「列表项右键菜单」）。
   *
   * 定位到文件是这里最常用的项 —— 用户找到想要的音效后，
   * 下一步通常就是去文件夹里把它拖进引擎或剪辑软件。
   */
  import { untrack } from "svelte";
  import type { AudioItem } from "../types";

  interface MenuItem {
    id: string;
    label: string;
    hint?: string;
    danger?: boolean;
    disabled?: boolean;
  }

  interface Props {
    /** 目标条目。 */
    item: AudioItem;
    /** 菜单左上角坐标（页面坐标系）。 */
    x: number;
    y: number;
    /** 选中数量，用于显示批量操作。 */
    selectionCount: number;
    onclose: () => void;
    onaction: (action: string) => void;
  }

  let { item, x, y, selectionCount, onclose, onaction }: Props = $props();

  /** 菜单项（用 $derived 以便随 selectionCount 变化）。 */
  const items = $derived<MenuItem[]>([
    { id: "play", label: "播放", hint: "双击列表同样可以播放" },
    { id: "reveal", label: "在文件夹中定位", hint: "打开所在目录并选中该文件" },
    { id: "copy_path", label: "复制完整路径" },
    { id: "copy_name", label: "复制文件名" },
    { id: "reanalyze", label: "重新分析", hint: "清空自动标签后重新生成" },
    {
      id: "analyze_pending",
      label: "分析全部未标注",
      hint: "为库中所有还没有自动标签的音频生成标签"
    },
    {
      id: "delete",
      label: selectionCount > 1 ? `从库中移除 ${selectionCount} 个` : "从库中移除",
      hint: "只移出库，不删除磁盘文件",
      danger: true
    }
  ]);

  /** 菜单元素，用于边界收敛。 */
  let el = $state<HTMLDivElement | null>(null);
  /** 调整后的位置，避免超出窗口。 */
  let pos = $state({ x: untrack(() => x), y: untrack(() => y) });

  $effect(() => {
    const node = el;
    // 读取 props 作为依赖，但初始值用 untrack 避免误报
    const px = x;
    const py = y;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    const maxX = window.innerWidth - rect.width - 6;
    const maxY = window.innerHeight - rect.height - 6;
    pos = {
      x: Math.max(6, Math.min(px, maxX)),
      y: Math.max(6, Math.min(py, maxY))
    };
  });

  /** 点击菜单项。 */
  function choose(id: string, disabled?: boolean): void {
    if (disabled) return;
    onaction(id);
    onclose();
  }
</script>

<svelte:window
  onclick={() => onclose()}
  oncontextmenu={() => onclose()}
  onkeydown={(e) => e.key === "Escape" && onclose()}
/>

<div
  class="menu"
  bind:this={el}
  style="left: {pos.x}px; top: {pos.y}px"
  role="menu"
  tabindex="-1"
  onclick={(e) => e.stopPropagation()}
  oncontextmenu={(e) => {
    e.preventDefault();
    e.stopPropagation();
  }}
  onkeydown={(e) => e.stopPropagation()}
>
  <div class="head" title={item.path}>{item.filename}</div>
  {#each items as it (it.id)}
    <button
      class="item"
      class:danger={it.danger}
      role="menuitem"
      onclick={() => choose(it.id, it.disabled)}
      disabled={it.disabled}
      title={it.hint ?? ""}
    >
      <span class="label">{it.label}</span>
    </button>
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 300;
    min-width: 194px;
    background: var(--panel-raised);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: var(--shadow-modal);
    padding: 4px;
    outline: none;
  }

  .head {
    padding: 6px 9px 7px;
    font-size: 11px;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border-soft);
    margin-bottom: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 260px;
  }

  .item {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    color: var(--text);
    font-family: inherit;
    font-size: 12.5px;
    padding: 6px 10px;
    border-radius: 5px;
    cursor: pointer;
  }

  .item:hover:not(:disabled) {
    background: var(--hover);
    color: var(--accent);
  }

  .item.danger:hover:not(:disabled) {
    color: var(--danger-fg);
  }

  .item:disabled {
    opacity: 0.45;
    cursor: default;
  }
</style>
