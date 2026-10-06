<script lang="ts">
  /**
   * 设置面板：外观（主题）、模型管理（一键下载 / 目录）、行为开关。
   *
   * 关于「点一下就关闭」的问题：面板根节点自身**不**绑定任何点击处理，
   * 关闭只有两条路径 —— 点遮罩层，或点右上角关闭按钮。
   * 之前把「点遮罩关闭」的处理器放在了包住整个面板的容器上，
   * 面板内部的点击会冒泡到该容器，于是点任意选项都会把面板关掉。
   * 现在遮罩与面板是兄弟节点，互不影响。
   */
  import { onMount } from "svelte";
  import { library } from "../stores.svelte";
  import { THEMES } from "../theme";
  import { formatBytes } from "../format";

  interface Props {
    /** 关闭面板。 */
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  onMount(() => {
    // 进度事件在 store 启动时已订阅，这里只刷新一次模型状态
    void library.refreshModels();
  });

  /** 模型元信息（按文件名取下载清单 / 状态）。 */
  function meta(name: string) {
    return library.modelDownloads.find((d) => d.name === name);
  }
  function status(name: string) {
    return library.modelsInfo?.models.find((m) => m.name === name);
  }

  /** 下载进度百分比。 */
  function percent(name: string): number {
    const p = library.downloadProgress[name];
    if (!p) return 0;
    if (p.phase === "done") return 100;
    if (p.ratio !== null) return Math.round(p.ratio * 100);
    return 0;
  }

  /** 速度展示。 */
  function speed(name: string): string {
    const p = library.downloadProgress[name];
    if (!p || p.bytesPerSec <= 0) return "";
    return `${formatBytes(p.bytesPerSec)}/s`;
  }

  /** 是否正在下载。 */
  function busy(name: string): boolean {
    const phase = library.downloadProgress[name]?.phase;
    return phase === "connecting" || phase === "downloading" || phase === "verifying";
  }

  /** 阶段中文名。 */
  const PHASE_LABEL: Record<string, string> = {
    connecting: "正在连接…",
    downloading: "下载中",
    verifying: "校验中…",
    done: "下载完成",
    failed: "下载失败",
    cancelled: "已取消"
  };
</script>

<!-- 遮罩：仅点击遮罩本身才关闭 -->
<div
  class="backdrop"
  onclick={() => onclose()}
  onkeydown={(e) => e.key === "Escape" && onclose()}
  role="presentation"
></div>

<div class="panel" role="dialog" aria-modal="true" aria-label="设置">
  <header>
    <h2>设置</h2>
    <button class="close" onclick={() => onclose()} aria-label="关闭设置">×</button>
  </header>

  <div class="body">
    <!-- ============ 外观 ============ -->
    <section>
      <h3>外观</h3>
      <p class="desc">
        全部主题都是低饱和度配色，专为长时间浏览素材设计。切换立即生效并自动保存。
      </p>
      <div class="themes">
        {#each THEMES as t (t.id)}
          <button
            class="theme"
            class:on={library.theme === t.id}
            data-theme-btn={t.id}
            onclick={() => library.setTheme(t.id)}
            aria-pressed={library.theme === t.id}
            title={t.hint}
          >
            <span class="swatch" aria-hidden="true">
              {#each t.swatch as c (c)}
                <span class="dot" style="background: {c}"></span>
              {/each}
            </span>
            <span class="tname">{t.label}</span>
            <span class="tscheme">{t.scheme === "dark" ? "深色" : "浅色"}</span>
          </button>
        {/each}
      </div>
    </section>

    <!-- ============ 标签语言 ============ -->
    <section>
      <h3>标签语言</h3>
      <p class="desc">
        同一段声音，用不同职业习惯的词来描述。<strong>数据库里存的始终是规范标签</strong>，
        这里只改变显示与搜索的说法 —— 切换不会丢标签，也不会影响用户自己写的标签。
      </p>
      <div class="vocabs">
        {#each library.vocab?.vocabularies ?? [] as v (v.id)}
          <button
            class="vocab"
            class:on={library.vocab?.current === v.id}
            data-vocab-btn={v.id}
            onclick={() => library.setVocab(v.id)}
            aria-pressed={library.vocab?.current === v.id}
            title={v.hint}
          >
            <span class="vname">{v.label}</span>
            <span class="vaud">{v.audience}</span>
          </button>
        {/each}
      </div>

      {#if library.vocab}
        {@const current = library.vocab.vocabularies.find((v) => v.id === library.vocab?.current)}
        {#if current}
          <p class="desc vhint">{current.hint}</p>
          <details class="maptable">
            <summary>查看术语对照表（{current.table.length} 项）</summary>
            <div class="mapgrid">
              {#each current.table as [canonical, shown] (canonical)}
                <div class="maprow">
                  <span class="canon">{canonical}</span>
                  <span class="arrow">→</span>
                  {#if library.vocab.current === "custom"}
                    <input
                      class="rename"
                      value={shown}
                      placeholder={canonical}
                      onchange={(e) =>
                        library.setVocabOverride(
                          canonical,
                          (e.currentTarget as HTMLInputElement).value.trim() || null
                        )}
                      aria-label="{canonical} 的说法"
                    />
                  {:else}
                    <span class="shown">{shown}</span>
                  {/if}
                </div>
              {/each}
            </div>
            {#if library.vocab.current === "custom"}
              <p class="hint-small">
                留空并移开焦点即可恢复默认说法。自定义说法会同时用于搜索，
                例如把「沉闷」改成「爆炸」后，搜「爆炸」就能命中。
              </p>
            {/if}
          </details>
        {/if}
      {/if}
    </section>

    <!-- ============ 行为 ============ -->
    <section>
      <h3>行为</h3>
      <label class="row">
        <input
          type="checkbox"
          checked={library.settings?.auto_analyze ?? true}
          onchange={(e) =>
            library.updateSettings({ auto_analyze: (e.currentTarget as HTMLInputElement).checked })}
        />
        <span>
          导入后自动在后台生成标签与波形
          <small>关闭后可在工具条点「分析未标注」手动触发</small>
        </span>
      </label>

      <label class="row">
        <input
          type="checkbox"
          checked={library.settings?.mode === "full"}
          onchange={(e) =>
            library.updateSettings({
              mode: (e.currentTarget as HTMLInputElement).checked ? "full" : "lite"
            })}
        />
        <span>
          完整模式（启用语义搜索入口）
          <small>轻量模式只用关键词 + 自动标签，速度最快</small>
        </span>
      </label>
    </section>

    <!-- ============ 模型 ============ -->
    <section>
      <h3>模型</h3>
      <p class="desc">
        模型按需下载，不随应用分发，也不会在启动时自动加载。
        下载完成后对应的智能功能自动生效。
      </p>

      {#if library.modelsInfo}
        <div class="status" class:ready={library.modelsInfo.semantic_ready}>
          {#if library.modelsInfo.semantic_ready}
            ✅ 推理内核已就绪，语义搜索可用。
            已向量化 {library.modelsInfo.embedded_audio} 个音频{library.modelsInfo
              .inference_sessions > 0
              ? `，已加载 ${library.modelsInfo.inference_sessions} 个模型到内存`
              : ""}。
          {:else}
            ⚠️ 语义搜索未启用：CLAP 模型尚未安装，当前使用关键词搜索
          {/if}
        </div>

        <div class="notice">
          <strong>说明：</strong>
          <em>ONNX Runtime 推理内核已静态链接进程序</em>，随安装包分发，
          <strong>不需要单独下载</strong>。这里需要下载的只是模型文件本身。
          <br />
          自动标签：
          {#if library.modelsInfo.tagging_ready}
            已启用（AST 音频事件分类，共 527 类）
          {:else}
            仅规则引擎 —— 下载 <code>yamnet.onnx</code> 可额外获得事件分类标签
          {/if}。
        </div>

        <div class="dir">
          <div class="dir-head">
            <span class="k">模型目录</span>
            {#if !library.modelsInfo.dir_writable}
              <span class="badge danger">不可写</span>
            {/if}
            {#if library.settings?.models_dir}
              <span class="badge">自定义</span>
            {:else}
              <span class="badge">默认</span>
            {/if}
          </div>
          <code class="path">{library.modelsInfo.models_dir}</code>
          <div class="dir-actions">
            <button class="mini" onclick={() => library.pickModelsDir()}>更改目录…</button>
            <button class="mini" onclick={() => library.resetModelsDir()}>恢复默认</button>
            <button class="mini" onclick={() => library.revealModelsDir()}>打开目录</button>
          </div>
          {#if library.settings?.models_dir}
            <p class="hint-small">默认位置：{library.modelsInfo.default_models_dir}</p>
          {/if}
        </div>

        <ul class="models">
          {#each library.modelsInfo.models as m (m.name)}
            {@const info = meta(m.name)}
            {@const st = status(m.name)}
            <li class:installed={m.installed}>
              <div class="mhead">
                <span class="mname">{m.display_name}</span>
                {#if m.installed}
                  <span class="msize">{formatBytes(m.actual_size)}</span>
                {:else if info}
                  <span class="msize">{formatBytes(info.size_bytes)}</span>
                {/if}
                {#if m.installed}
                  {#if m.up_to_date}
                    <span class="badge ok">已安装</span>
                  {:else}
                    <span class="badge warn" title="磁盘上的文件大小与清单登记的不一致：可能是量化版或旧版本。建议删除后重新下载。">
                      版本不一致
                    </span>
                  {/if}
                {:else}
                  <span class="badge">未安装</span>
                {/if}
                {#if m.name === "yamnet.onnx"}
                  <span class="badge tag-jie">可选</span>
                {:else}
                  <span class="badge tag-sem">语义必需</span>
                {/if}
              </div>
              <p class="purpose">{m.purpose}</p>

              {#if m.installed && !m.up_to_date && info}
                <p class="mismatch">
                  磁盘上实际 <strong>{formatBytes(m.actual_size)}</strong>，清单登记
                  <strong>{formatBytes(info.size_bytes)}</strong>。
                  常见原因是装了 INT8 量化版 —— 它会让检索准确度明显下降
                  （实测命中率 1/6 vs 2/3）。建议「删除文件」后重新下载。
                </p>
              {/if}

              {#if busy(m.name)}
                <div class="progress-block">
                  <div class="progress">
                    <div class="bar" style="width: {percent(m.name)}%"></div>
                  </div>
                  <div class="prow">
                    <span>
                      {PHASE_LABEL[library.downloadProgress[m.name]?.phase ?? ""] ?? ""}
                      · {percent(m.name)}%
                    </span>
                    <span>
                      {formatBytes(library.downloadProgress[m.name]?.downloaded ?? 0)}
                      {#if library.downloadProgress[m.name]?.total}
                        / {formatBytes(library.downloadProgress[m.name]?.total ?? 0)}
                      {/if}
                      {speed(m.name)}
                    </span>
                    <button class="mini danger" onclick={() => library.cancelDownload(m.name)}>
                      取消
                    </button>
                  </div>
                </div>
              {:else if library.downloadProgress[m.name]?.phase === "failed"}
                <p class="err">下载失败：{library.downloadProgress[m.name]?.error}</p>
              {/if}

              <div class="mactions">
                {#if !m.installed}
                  <button
                    class="primary"
                    onclick={() => library.downloadModel(m.name)}
                    disabled={busy(m.name)}
                  >
                    {busy(m.name) ? "下载中…" : "一键下载"}
                  </button>
                {:else}
                  {#if m.loaded}
                    <button class="mini" onclick={() => library.unloadModel(m.name)}>卸载</button>
                  {/if}
                  <button class="mini danger" onclick={() => library.deleteModel(m.name)}>
                    删除文件
                  </button>
                {/if}
                {#if info}
                  <button class="mini" onclick={() => navigator.clipboard.writeText(info.url)}>
                    复制下载地址
                  </button>
                {/if}
              </div>
            </li>
          {/each}
        </ul>

        <details class="howto">
          <summary>离线安装 / 网络受限时怎么办？</summary>
          <ol>
            <li>在能联网的机器上打开「复制下载地址」得到的链接，保存文件。</li>
            <li>把文件重命名为左侧对应文件名，放入上面的模型目录。</li>
            <li>回到本页点「刷新」，或重启应用 —— 状态会变为「已安装」。</li>
          </ol>
          <p class="hint-small">
            提示：模型来自 Hugging Face，国内网络可能较慢，可在应用外使用下载工具；
            系统会自动尝试镜像地址。
          </p>
        </details>

        <div class="refresh-row">
          <button class="mini" onclick={() => library.refreshModels()}>刷新状态</button>
        </div>
      {:else}
        <p class="desc">正在读取模型状态…</p>
      {/if}
    </section>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--overlay);
    z-index: 100;
  }

  .panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 101;
    width: min(700px, 92vw);
    max-height: 88vh;
    display: flex;
    flex-direction: column;
    background: var(--panel-raised);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: var(--shadow-modal);
    overflow: hidden;
  }

  header {
    display: flex;
    align-items: center;
    padding: 13px 16px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  header h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .close {
    margin-left: auto;
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 20px;
    line-height: 1;
    cursor: pointer;
    padding: 0 4px;
  }

  .close:hover {
    color: var(--text);
  }

  .body {
    overflow-y: auto;
    padding: 4px 16px 18px;
  }

  section {
    padding: 14px 0;
    border-bottom: 1px solid var(--border-soft);
  }

  section:last-child {
    border-bottom: none;
  }

  h3 {
    margin: 0 0 8px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-dim);
    font-weight: 600;
  }

  .desc {
    margin: 0 0 10px;
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.65;
  }

  /* ---------- 主题选择 ---------- */
  .themes {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px;
  }

  .theme {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 9px 10px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    transition: border-color 0.12s;
  }

  .theme:hover {
    border-color: var(--accent);
  }

  .theme.on {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .swatch {
    display: flex;
    gap: 3px;
  }

  .dot {
    width: 15px;
    height: 15px;
    border-radius: 4px;
    border: 1px solid var(--border);
  }

  .tname {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }

  .tscheme {
    font-size: 11px;
    color: var(--text-dim);
  }

  /* ---------- 词汇表 ---------- */
  .vocabs {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 8px;
  }

  .vocab {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 9px 10px;
    background: var(--input-bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    text-align: left;
    font-family: inherit;
    transition: border-color 0.12s;
  }

  .vocab:hover {
    border-color: var(--accent);
  }

  .vocab.on {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .vname {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }

  .vaud {
    font-size: 11px;
    color: var(--text-dim);
  }

  .vhint {
    margin-top: 9px;
  }

  .maptable {
    margin-top: 8px;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    padding: 9px 12px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .maptable summary {
    cursor: pointer;
    color: var(--text);
  }

  .mapgrid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
    gap: 5px 14px;
    margin-top: 9px;
  }

  .maprow {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
  }

  .canon {
    color: var(--text-dim);
    min-width: 62px;
  }

  .arrow {
    color: var(--text-dim);
    opacity: 0.6;
  }

  .shown {
    color: var(--text);
  }

  .rename {
    flex: 1;
    min-width: 0;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 5px;
    color: var(--text);
    padding: 3px 7px;
    font-size: 12px;
    font-family: inherit;
    outline: none;
  }

  .rename:focus {
    border-color: var(--accent);
  }

  /* ---------- 行为 ---------- */
  .row {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    padding: 7px 0;
    font-size: 13px;
    cursor: pointer;
  }

  .row input {
    margin-top: 2px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .row span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .row small {
    font-size: 11px;
    color: var(--text-dim);
  }

  /* ---------- 模型 ---------- */
  .status {
    padding: 8px 11px;
    border-radius: var(--radius);
    font-size: 12px;
    background: var(--warn-bg);
    color: var(--warn-fg);
    border: 1px solid var(--warn-border);
    margin-bottom: 10px;
  }

  .status.ready {
    background: var(--ok-bg);
    color: var(--ok-fg);
    border-color: var(--ok-border);
  }

  .notice {
    padding: 9px 11px;
    border-radius: var(--radius);
    font-size: 12px;
    line-height: 1.65;
    background: var(--info-bg);
    color: var(--info-fg);
    border: 1px solid var(--info-border);
    margin-bottom: 10px;
  }

  .notice em {
    font-style: normal;
    font-weight: 600;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .dir {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin-bottom: 10px;
    background: var(--input-bg);
  }

  .dir-head {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-bottom: 6px;
  }

  .dir-head .k {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
  }

  .path {
    display: block;
    word-break: break-all;
    font-size: 11px;
    color: var(--text);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 8px;
    margin-bottom: 8px;
  }

  .dir-actions,
  .mactions,
  .refresh-row {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .hint-small {
    margin: 6px 0 0;
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.6;
    word-break: break-all;
  }

  .models {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 9px;
  }

  .models li {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    background: var(--panel);
  }

  .models li.installed {
    border-color: var(--ok-border);
  }

  .mhead {
    display: flex;
    align-items: center;
    gap: 7px;
    flex-wrap: wrap;
  }

  .mname {
    font-size: 13px;
    font-weight: 600;
  }

  .msize {
    font-size: 11px;
    color: var(--text-dim);
  }

  .badge {
    font-size: 10px;
    padding: 1px 7px;
    border-radius: 999px;
    background: var(--chip-bg);
    color: var(--text-dim);
    border: 1px solid var(--border);
  }

  .badge.ok {
    background: var(--ok-bg);
    color: var(--ok-fg);
    border-color: var(--ok-border);
  }

  .badge.danger {
    background: var(--danger-bg);
    color: var(--danger-fg);
    border-color: var(--danger-border);
  }

  .badge.tag-sem {
    background: var(--tag-user-bg);
    color: var(--tag-user-fg);
    border-color: var(--tag-user-border);
  }

  .badge.tag-jie {
    background: var(--tag-inherited-bg);
    color: var(--tag-inherited-fg);
    border-color: var(--tag-inherited-border);
  }

  .purpose {
    margin: 5px 0 9px;
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.6;
  }

  .progress-block {
    margin-bottom: 9px;
  }

  .progress {
    height: 5px;
    background: var(--border);
    border-radius: 999px;
    overflow: hidden;
    margin-bottom: 5px;
  }

  .bar {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s ease;
  }

  .prow {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 11px;
    color: var(--text-dim);
    flex-wrap: wrap;
  }

  .prow button {
    margin-left: auto;
  }

  .err {
    margin: 0 0 9px;
    font-size: 11px;
    color: var(--danger-fg);
    line-height: 1.6;
  }

  button.primary {
    background: var(--accent);
    border: 1px solid var(--accent);
    color: var(--accent-contrast);
    font-weight: 600;
    border-radius: 6px;
    padding: 5px 14px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
  }

  button.primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }

  button.primary:disabled {
    opacity: 0.55;
    cursor: default;
  }

  .mini {
    background: var(--chip-bg);
    border: 1px solid var(--border);
    color: var(--text-dim);
    border-radius: 6px;
    padding: 4px 10px;
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
    border-color: var(--danger-border);
  }

  .howto {
    margin-top: 10px;
    border: 1px dashed var(--border);
    border-radius: var(--radius);
    padding: 9px 12px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .howto summary {
    cursor: pointer;
    color: var(--text);
  }

  .howto ol {
    margin: 8px 0;
    padding-left: 18px;
    line-height: 1.8;
  }

  .refresh-row {
    margin-top: 10px;
  }
</style>
