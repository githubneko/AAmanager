<script lang="ts">
  /**
   * 备注编辑（策划书 2.1 / 7.2）：多行文本，自动保存，支持追加。
   */
  import { untrack } from "svelte";

  interface Props {
    /** 目标音频 id。 */
    audioId: number;
    /** 当前备注内容。 */
    note: string;
    /** 保存回调。 */
    onsave: (note: string) => Promise<void> | void;
    /** 保存按钮文案。 */
    saveLabel?: string;
  }

  let { audioId, note, onsave, saveLabel = "保存" }: Props = $props();

  /** 编辑中的草稿。初始值用 untrack 读取，之后由 effect 同步。 */
  let draft = $state(untrack(() => note));
  /** 保存状态提示。 */
  let status = $state<"idle" | "saving" | "saved">("idle");
  /** 是否有未保存的修改。 */
  const dirty = $derived(draft !== note);

  /**
   * 同步草稿：
   * * 切换到另一个音频 —— 直接采用新备注；
   * * 同一个音频但备注被外部改动（例如后台生成了自动备注）—— 仅在没有
   *   本地未保存修改时同步，避免覆盖用户正在输入的内容。
   *
   * 用 `untrack` 读取 `draft`，让本 effect 只依赖 `audioId` / `note`，
   * 否则「读 draft 又写 draft」会形成自触发循环。
   */
  let lastId = $state(untrack(() => audioId));
  $effect(() => {
    const id = audioId;
    const external = note;
    untrack(() => {
      if (id !== lastId) {
        lastId = id;
        draft = external;
        status = "idle";
      } else if (draft !== external && !dirty) {
        draft = external;
      }
    });
  });

  /** 保存。 */
  async function save(): Promise<void> {
    status = "saving";
    await onsave(draft);
    status = "saved";
    setTimeout(() => {
      if (status === "saved") status = "idle";
    }, 1600);
  }

  /** 撤销。 */
  function revert(): void {
    draft = note;
    status = "idle";
  }
</script>

<div class="note-editor">
  <textarea
    bind:value={draft}
    rows="3"
    placeholder="写点备注，例如：适合用在金属撞击的过场，注意尾部混响"
    onkeydown={(e) => {
      // Ctrl/Cmd + Enter 保存
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        void save();
      }
    }}
    aria-label="音频备注"
  ></textarea>

  <div class="actions">
    <button class="save" onclick={save} disabled={!dirty || status === "saving"}>
      {status === "saving" ? "保存中…" : saveLabel}
    </button>
    <button class="revert" onclick={revert} disabled={!dirty}>撤销</button>
    {#if status === "saved"}
      <span class="ok">已保存</span>
    {:else if dirty}
      <span class="dirty">未保存</span>
    {/if}
    <span class="hint">Ctrl+Enter 保存</span>
  </div>
</div>

<style>
  .note-editor {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    min-height: 62px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: 7px;
    color: var(--text);
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.6;
    font-family: inherit;
    outline: none;
  }

  textarea:focus {
    border-color: var(--accent);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  button {
    border-radius: 6px;
    padding: 5px 12px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    border: 1px solid var(--border);
    background: var(--chip-bg);
    color: var(--text);
  }

  button.save {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
  }

  button:hover:not(:disabled) {
    filter: brightness(1.12);
  }

  button:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .ok {
    font-size: 11px;
    color: var(--tag-confirmed-fg);
  }

  .dirty {
    font-size: 11px;
    color: var(--warn-fg);
  }

  .hint {
    margin-left: auto;
    font-size: 11px;
    color: var(--text-dim);
    opacity: 0.7;
  }
</style>
