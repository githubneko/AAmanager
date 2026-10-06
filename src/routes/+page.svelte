<script lang="ts">
  /**
   * 主界面（策划书 7.1）：搜索栏 + 筛选面板 + 音频列表 + 波形 / 标签 / 备注。
   */
  import { onMount, onDestroy } from "svelte";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import SearchBar from "$lib/components/SearchBar.svelte";
  import FilterPanel from "$lib/components/FilterPanel.svelte";
  import AudioList from "$lib/components/AudioList.svelte";
  import AudioPlayer from "$lib/components/AudioPlayer.svelte";
  import TagEditor from "$lib/components/TagEditor.svelte";
  import NoteEditor from "$lib/components/NoteEditor.svelte";
  import BulkEditBar from "$lib/components/BulkEditBar.svelte";
  import SettingsPanel from "$lib/components/SettingsPanel.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import { library } from "$lib/stores.svelte";
  import * as api from "$lib/api";
  import type { AudioItem } from "$lib/types";
  import {
    formatBytes,
    formatChannels,
    formatSampleRate,
    formatDuration,
    dirName
  } from "$lib/format";

  /** 是否显示设置 / 模型面板。 */
  let showModels = $state(false);
  /** 拖拽悬停中（显示投放提示）。 */
  let dragging = $state(false);
  /** 详情面板是否可见。 */
  let showDetails = $state(true);
  /** 右键菜单状态。 */
  let menu = $state<{ item: AudioItem; x: number; y: number } | null>(null);

  /** 处理右键菜单里的动作。 */
  async function handleMenuAction(action: string): Promise<void> {
    const item = menu?.item;
    if (!item) return;
    switch (action) {
      case "play":
        await library.play(item.id);
        break;
      case "reveal":
        await library.reveal(item.id);
        break;
      case "copy_path":
        try {
          await navigator.clipboard.writeText(item.path);
        } catch {
          library.error = "复制失败：剪贴板不可用";
        }
        break;
      case "copy_name":
        try {
          await navigator.clipboard.writeText(item.filename);
        } catch {
          library.error = "复制失败：剪贴板不可用";
        }
        break;
      case "reanalyze":
        await library.reanalyzeSelection();
        break;
      case "analyze_pending":
        await library.reanalyzeSelection(true);
        break;
      case "delete":
        await library.deleteSelection();
        break;
    }
  }

  onMount(async () => {
    // 启动时不加载任何模型，只读数据库并渲染列表（策划书 2.3）
    await library.init();
    library.startIndexPolling();

    // 开发期：按环境变量指定目录自动导入，用于端到端验证「导入 → 分析 → 渲染」
    if (import.meta.env.DEV) {
      const dir = await api.debugAutoImportDir().catch(() => null);
      if (dir) {
        await library.importPaths([dir]);
        library.startIndexPolling();
        // 等后台分析排空，让自述里的行数反映分析后的最终状态
        for (let i = 0; i < 100; i++) {
          await new Promise((r) => setTimeout(r, 200));
          if (library.indexing.pending === 0 && !library.indexing.running) break;
        }
        await library.refresh();
      }
    }

    // 开发期：把渲染结果回传后端落盘，供自动化脚本断言界面真的渲染出来了。
    // 生产构建不含该命令，因此用 DEV 兜住。
    if (import.meta.env.DEV) {
      try {
        await api.debugReportUi({
          mounted: !!document.querySelector(".app"),
          title: document.title,
          rows: document.querySelectorAll(".rows .row").length,
          empty_hint: document.body.innerText.includes("音频库还是空的"),
          has_search: !!document.querySelector("input[type=text]"),
          has_filters: !!document.querySelector("aside.filters"),
          has_toolbar: ["导入文件夹", "导入文件", "分析未标注", "全选"].every((t) =>
            document.body.innerText.includes(t)
          ),
          init_ok: library.error === null,
          error: library.error,
          total: library.total,
          search_mode: library.searchMode,
          app_mode: library.settings?.mode ?? "unknown"
        });
      } catch (e) {
        console.warn("渲染自述上报失败（生产构建属正常）:", e);
      }
    }

    // 播放进度由 AudioPlayer 自己的 rAF + 自适应轮询驱动（此处不再重复轮询）

    // 开发期：定时落盘页面状态，供真实键盘输入的自动化验证读取
    if (import.meta.env.DEV) {
      // 暴露少量开发期钩子，便于自动化验证主题切换与设置面板行为
      const hooks = window as unknown as Record<string, unknown>;
      hooks.__aam_set_theme = (id: string) => library.setTheme(id);
      hooks.__aam_toggle_settings = (open: boolean) => {
        showModels = open;
        return showModels;
      };
      hooks.__aam_click_settings_theme = (id: string) => {
        const btn = document.querySelector<HTMLButtonElement>(`[data-theme-btn="${id}"]`);
        btn?.click();
        return !!btn;
      };

      // AAM_PROBE_FOCUS=1 时自动聚焦搜索框，用于把「键盘可用性」与
      // 「鼠标点击命中」两个问题分开验证
      if (await api.debugShouldFocusSearch().catch(() => false)) {
        document.querySelector<HTMLInputElement>("input[type=text]")?.focus();
      }

      // 命令轮询：外部脚本通过文件队列驱动界面（替代不可用的 CDP）
      type Cmd = { op: string; id?: number; [k: string]: unknown };
      /** 最近一次「定位到文件」的结果（开发期验证用）。 */
      let lastReveal: Record<string, unknown> | null = null;
      const runCommand = (cmd: Cmd): unknown => {
        switch (cmd.op) {
          case "set_theme":
            void library.setTheme(String(cmd.theme));
            return { queued: true, theme: cmd.theme };
          case "open_settings":
            showModels = true;
            return { open: showModels };
          case "close_settings":
            showModels = false;
            return { open: showModels };
          case "click_theme_button": {
            const btn = document.querySelector<HTMLButtonElement>(
              `[data-theme-btn="${String(cmd.theme)}"]`
            );
            btn?.click();
            return { found: !!btn };
          }
          case "click_settings_open": {
            // 点工具条上的设置按钮（真实交互路径）
            const btns = Array.from(document.querySelectorAll<HTMLButtonElement>("button"));
            const b = btns.find((x) => (x.getAttribute("title") ?? "").includes("设置"));
            b?.click();
            return { found: !!b };
          }
          case "click_at": {
            // 在面板内部某点上派发真实 DOM 点击，验证冒泡是否导致关闭
            const el = document.elementFromPoint(Number(cmd.x), Number(cmd.y));
            el?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
            return { tag: el?.tagName ?? null, cls: (el as HTMLElement)?.className ?? null };
          }
          case "click_backdrop": {
            // 点左上角（一定在遮罩上），应当关闭面板
            const el = document.elementFromPoint(8, 8);
            el?.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
            return { tag: el?.tagName ?? null, cls: (el as HTMLElement)?.className ?? null };
          }
          case "scroll_settings": {
            const body = document.querySelector<HTMLElement>(".body");
            if (body) body.scrollTop = Number(cmd.y ?? 400);
            return { scrolled: body?.scrollTop ?? null };
          }
          case "reveal": {
            // 记录调用前后的 error，便于验证脚本判断成功与否
            const before = library.error;
            void library.reveal(Number(cmd.audio_id)).then(() => {
              lastReveal = {
                id: Number(cmd.audio_id),
                before,
                after: library.error,
                ok: library.error === before
              };
            });
            return { started: cmd.audio_id };
          }
          case "reveal_result":
            return lastReveal ?? { pending: true };
          case "inherit_tags":
            void library.inheritTags();
            return { queued: true };
          case "set_vocab":
            void library.setVocab(String(cmd.vocab));
            return { queued: true, vocab: cmd.vocab };
          case "snapshot":
            return {
              theme: library.theme,
              settingsOpen: showModels,
              attr: document.documentElement.getAttribute("data-theme"),
              error: library.error
            };
          case "reveal_await": {
            // 等待结果版本：验证脚本需要知道调用成功还是失败
            return { pending: true };
          }
          case "download_model":
            // 走与「一键下载」按钮完全相同的路径
            void library.downloadModel(String(cmd.name));
            return { started: cmd.name };
          case "download_state":
            return {
              events: library.downloadEvents,
              progress: Object.fromEntries(
                Object.entries(library.downloadProgress).map(([k, v]) => [
                  k,
                  { phase: v.phase, ratio: v.ratio, downloaded: v.downloaded, error: v.error }
                ])
              ),
              models: library.modelsInfo?.models.map((m) => ({
                name: m.name,
                installed: m.installed,
                actual_size: m.actual_size
              })),
              dir: library.modelsInfo?.models_dir
            };
          default:
            return { error: `unknown op: ${cmd.op}` };
        }
      };

      setInterval(() => {
        void api
          .debugTakeCommands()
          .then(async (cmds) => {
            if (!cmds || cmds.length === 0) return;
            const results = cmds.map((c) => ({
              id: (c as Cmd).id ?? null,
              result: runCommand(c as Cmd)
            }));
            // 等一帧让 Svelte 完成渲染，脚本读状态时才是最新值
            await new Promise((r) => setTimeout(r, 250));
            await api
              .debugWriteCommandResult({
                at: Date.now(),
                results,
                theme: library.theme,
                settingsOpen: showModels,
                attr: document.documentElement.getAttribute("data-theme"),
                bg: getComputedStyle(document.documentElement).getPropertyValue("--bg").trim(),
                accent: getComputedStyle(document.documentElement)
                  .getPropertyValue("--accent")
                  .trim()
              })
              .catch(() => {});
          })
          .catch(() => {});
      }, 300);

      // 记录输入框上真实到达的事件，用于判断按键是否进入了视图
      const events: string[] = [];
      const probe = document.querySelector<HTMLInputElement>("input[type=text]");
      for (const type of ["keydown", "keypress", "beforeinput", "input", "focus", "blur"]) {
        probe?.addEventListener(type, (e) => {
          const ke = e as KeyboardEvent & { data?: string };
          events.push(`${type}:${ke.key ?? ke.data ?? ""}`);
          if (events.length > 60) events.shift();
        });
      }
      // 也记录 window 上到达的按键，区分「事件没到视图」与「事件没到输入框」
      const winEvents: string[] = [];
      window.addEventListener("keydown", (e) => {
        winEvents.push(`win-keydown:${e.key}`);
        if (winEvents.length > 60) winEvents.shift();
      });

      setInterval(() => {
        const input = document.querySelector<HTMLInputElement>("input[type=text]");
        const rect = input?.getBoundingClientRect();
        const cs = input ? getComputedStyle(input) : null;
        void api
          .debugWritePageState({
            at: Date.now(),
            activeElement: document.activeElement?.tagName ?? null,
            activeIsSearch: document.activeElement === input,
            inputValue: input?.value ?? null,
            storeQueryText: library.query.text,
            rows: document.querySelectorAll(".rows .row").length,
            total: library.total,
            loading: library.loading,
            error: library.error,
            devicePixelRatio: window.devicePixelRatio,
            clientWidth: document.documentElement.clientWidth,
            clientHeight: document.documentElement.clientHeight,
            inputEvents: [...events],
            windowKeyEvents: [...winEvents],
            hasFocus: document.hasFocus(),
            theme: library.theme,
            settingsOpen: showModels,
            rootThemeAttr: document.documentElement.getAttribute("data-theme"),
            bgColor: getComputedStyle(document.documentElement).getPropertyValue("--bg").trim(),
            accentColor: getComputedStyle(document.documentElement)
              .getPropertyValue("--accent")
              .trim(),
            // 输入框在页面坐标系中的位置与关键样式，便于外部脚本判断点击是否命中
            inputRect: rect
              ? { x: rect.x, y: rect.y, w: rect.width, h: rect.height }
              : null,
            inputStyle: cs
              ? {
                  pointerEvents: cs.pointerEvents,
                  visibility: cs.visibility,
                  display: cs.display,
                  opacity: cs.opacity,
                  caretColor: cs.caretColor,
                  zIndex: cs.zIndex
                }
              : null
          })
          .catch(() => {});
      }, 500);
    }

    // 原生拖拽事件：Tauri 会把文件系统的真实路径告诉我们
    try {
      const webview = getCurrentWebview();
      await webview.onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          dragging = true;
        } else if (payload.type === "leave") {
          dragging = false;
        } else if (payload.type === "drop") {
          dragging = false;
          const paths = payload.paths ?? [];
          if (paths.length > 0) void library.importPaths(paths);
        }
      });
    } catch (e) {
      // 非 Tauri 环境（例如浏览器里跑 vite dev）下没有该 API
      console.warn("拖拽导入不可用：", e);
    }
  });

  onDestroy(() => {
    library.stopIndexPolling();
  });

  /** 焦点条目。 */
  const focused = $derived(library.focused);
</script>

<div class="app" class:dragging>
  <SearchBar onsettings={() => (showModels = !showModels)} />

  <!-- 工具条 -->
  <div class="toolbar">
    <button class="tb" onclick={() => library.importFolder()}>📁 导入文件夹</button>
    <button class="tb" onclick={() => library.importFiles()}>📄 导入文件</button>
    <button
      class="tb"
      onclick={() => library.reanalyzeSelection(true)}
      title="为所有尚无自动标签的音频生成标签与备注"
    >
      🏷️ 分析未标注
    </button>
    <button class="tb" onclick={() => library.selectAll()} title="全选当前结果">
      ☑ 全选
    </button>
    <button
      class="tb"
      onclick={() => library.revealSelection()}
      disabled={library.focusedId === null}
      title="在系统文件管理器中打开所在文件夹并选中该文件"
    >
      📂 定位到文件
    </button>
    <button
      class="tb"
      onclick={() => library.inheritTags()}
      title="把相似音频上你自己打的标签传播过来（相似度 ≥ 0.85）"
    >
      🔗 继承相似标签
    </button>

    <span class="spacer"></span>

    {#if library.indexing.pending > 0 || library.indexing.running}
      <span class="indexing" title="后台正在分析音频，不影响继续操作">
        <span class="dot"></span>
        后台分析中 · 剩余 {library.indexing.pending}
        {#if library.indexing.done > 0}· 已完成 {library.indexing.done}{/if}
        {#if library.indexing.failed > 0}· 失败 {library.indexing.failed}{/if}
      </span>
    {/if}

    <span class="stat" title="库内音频总数">共 {library.stats.total} 个</span>
    <span class="stat" title="标签种类数">{library.stats.tag_count} 个标签</span>

    <button
      class="tb icon"
      class:on={showDetails}
      onclick={() => (showDetails = !showDetails)}
      title="显示 / 隐藏详情面板"
    >
      ▤
    </button>
    <button
      class="tb"
      class:on={library.settings?.mode === "full"}
      onclick={() =>
        library.updateSettings({
          mode: library.settings?.mode === "full" ? "lite" : "full"
        })}
      title="轻量模式：仅关键词 + 自动标签；完整模式：加入语义搜索"
    >
      {library.settings?.mode === "full" ? "完整模式" : "轻量模式"}
    </button>
  </div>

  <!-- 提示横幅 -->
  {#if library.error}
    <div class="banner error">
      <span>{library.error}</span>
      <button onclick={() => library.clearError()} aria-label="关闭">×</button>
    </div>
  {/if}

  {#if library.notice}
    <div class="banner info">
      <span>{library.notice}</span>
      <button onclick={() => library.clearNotice()} aria-label="关闭">×</button>
    </div>
  {/if}

  {#if library.lastImport}
    <div class="banner ok">
      <span>
        导入完成：新增 {library.lastImport.imported} 个
        {#if library.lastImport.skipped > 0}
          · 跳过重复 {library.lastImport.skipped} 个
        {/if}
        {#if library.lastImport.failed > 0}
          · 无法解析 {library.lastImport.failed} 个
        {/if}
        {#if library.settings?.auto_analyze}
          · 已在后台开始生成标签
        {/if}
      </span>
      <button onclick={() => library.clearImportNotice()} aria-label="关闭">×</button>
    </div>
  {/if}

  <div class="main">
    <FilterPanel />

    <AudioList
      onimport={() => library.importFolder()}
      oncontextmenu={(item, x, y) => (menu = { item, x, y })}
    />

    {#if showDetails}
      <aside class="details">
        {#if focused}
          <h2 class="title" title={focused.path}>{focused.filename}</h2>
          {#if !focused.exists}
            <p class="missing-warn">⚠️ 磁盘上已找不到该文件</p>
          {/if}

          <dl class="meta">
            <div><dt>时长</dt><dd>{formatDuration(focused.duration)}</dd></div>
            <div><dt>采样率</dt><dd>{formatSampleRate(focused.sample_rate)}</dd></div>
            <div><dt>声道</dt><dd>{formatChannels(focused.channels)}</dd></div>
            <div><dt>格式</dt><dd>{(focused.format ?? "—").toUpperCase()}</dd></div>
            <div><dt>大小</dt><dd>{formatBytes(focused.file_size)}</dd></div>
            <div>
              <dt>码率</dt>
              <dd>{focused.bitrate ? `${Math.round(focused.bitrate / 1000)} kbps` : "—"}</dd>
            </div>
          </dl>

          <div class="path" title={focused.path}>
            <span class="k">目录</span>
            <span class="v">{dirName(focused.path) || "—"}</span>
          </div>

          <section class="block">
            <h3>标签</h3>
            <TagEditor audioId={focused.id} tags={focused.tags} />
          </section>

          <section class="block">
            <h3>备注</h3>
            <NoteEditor
              audioId={focused.id}
              note={focused.note}
              onsave={(n) => library.saveNote(focused.id, n)}
            />
          </section>

          <section class="block actions">
            <button class="mini" onclick={() => library.play(focused.id)}>播放</button>
            <button class="mini" onclick={() => library.reanalyzeSelection()}>重新分析</button>
            <button class="mini danger" onclick={() => library.deleteOne(focused.id)}>
              从库中移除
            </button>
          </section>
        {:else}
          <p class="hint">从列表中选择一个音频查看详情</p>
        {/if}
      </aside>
    {/if}
  </div>

  {#if library.selected.length > 1}
    <BulkEditBar />
  {/if}

  <AudioPlayer item={focused} />

  {#if showModels}
    <!-- SettingsPanel 自带遮罩与面板两个兄弟节点，面板内部的点击不会
         冒泡到遮罩，因此不会再出现「点一下就关闭」的问题 -->
    <SettingsPanel onclose={() => (showModels = false)} />
  {/if}

  {#if menu}
    <ContextMenu
      item={menu.item}
      x={menu.x}
      y={menu.y}
      selectionCount={Math.max(1, library.selected.length)}
      onclose={() => (menu = null)}
      onaction={handleMenuAction}
    />
  {/if}

  {#if dragging}
    <div class="drop-overlay">
      <div class="drop-box">
        <p class="big">松开即可导入</p>
        <p>支持文件夹与音频文件（WAV / MP3 / OGG / FLAC / M4A / AIFF）</p>
      </div>
    </div>
  {/if}
</div>

<style>
  .app {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 14px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    flex-wrap: wrap;
  }

  .tb {
    background: var(--input-bg);
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 7px;
    padding: 6px 11px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    white-space: nowrap;
  }

  .tb:hover {
    color: var(--text);
    border-color: var(--accent);
  }

  .tb.on {
    color: var(--accent);
    border-color: var(--accent);
  }

  .tb.icon {
    padding: 6px 10px;
  }

  .spacer {
    flex: 1;
  }

  .indexing {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--warn-fg);
    font-size: 11px;
    background: var(--warn-bg);
    border: 1px solid var(--tag-low-border);
    border-radius: 999px;
    padding: 3px 10px;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn-fg);
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.25;
    }
  }

  .stat {
    color: var(--text-dim);
    font-size: 11px;
    white-space: nowrap;
  }

  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 14px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }

  .banner span {
    flex: 1;
  }

  .banner button {
    background: none;
    border: none;
    color: inherit;
    font-size: 16px;
    cursor: pointer;
    line-height: 1;
    opacity: 0.7;
  }

  .banner.error {
    background: var(--danger-bg);
    color: var(--danger-fg);
  }

  .banner.info {
    background: var(--info-bg);
    color: var(--tag-user-fg);
  }

  .banner.ok {
    background: var(--ok-bg);
    color: var(--tag-confirmed-fg);
  }

  .main {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .details {
    width: 330px;
    flex-shrink: 0;
    overflow-y: auto;
    background: var(--panel);
    border-left: 1px solid var(--border);
    padding: 14px;
  }

  .title {
    font-size: 14px;
    margin: 0 0 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .missing-warn {
    margin: 0 0 10px;
    font-size: 12px;
    color: var(--danger-fg);
  }

  .meta {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 7px 12px;
    margin: 0 0 12px;
    font-size: 12px;
  }

  .meta > div {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .meta dt {
    color: var(--text-dim);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .meta dd {
    margin: 0;
    font-variant-numeric: tabular-nums;
  }

  .path {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-bottom: 14px;
    font-size: 11px;
  }

  .path .k {
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-size: 10px;
  }

  .path .v {
    color: var(--text-dim);
    word-break: break-all;
    line-height: 1.5;
  }

  .block {
    border-top: 1px solid var(--border);
    padding-top: 12px;
    margin-bottom: 14px;
  }

  .block h3 {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
    margin: 0 0 9px;
    font-weight: 600;
  }

  .block.actions {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .mini {
    background: var(--chip-bg);
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 6px;
    padding: 5px 10px;
    font-size: 11px;
    cursor: pointer;
    font-family: inherit;
  }

  .mini:hover {
    color: var(--text);
    border-color: var(--accent);
  }

  .mini.danger:hover {
    color: var(--danger-fg);
    border-color: var(--danger-fg);
  }

  .hint {
    color: var(--text-dim);
    font-size: 12px;
    text-align: center;
    margin-top: 40px;
  }

  .drop-overlay {
    position: fixed;
    inset: 0;
    background: var(--overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
    pointer-events: none;
  }

  .drop-box {
    border: 2px dashed var(--accent);
    border-radius: 16px;
    padding: 46px 62px;
    text-align: center;
    color: var(--text);
  }

  .drop-box .big {
    font-size: 19px;
    font-weight: 600;
    margin: 0 0 8px;
  }

  .drop-box p:last-child {
    margin: 0;
    color: var(--text-dim);
    font-size: 12px;
  }
</style>
