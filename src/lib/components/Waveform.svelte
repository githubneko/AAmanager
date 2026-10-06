<script lang="ts">
  /**
   * 波形峰值显示（Canvas 绘制）。
   *
   * 后端把整段音频压缩为固定数量的峰值（默认 1024 个 0-255 的字节），
   * 因此这里只需把它们画成竖线，并用「点序号 × 时长 / 点数」把点击位置
   * 换算成播放时间，实现策划书 7.2 要求的「点击波形跳转」。
   *
   * 配色从当前主题的 CSS 变量读取（`waveformTheme()`），因此换主题时会重绘。
   */
  import { waveformTheme } from "../theme";
  interface Props {
    /** 峰值数据（0-255）。 */
    peaks: number[];
    /** 音频总时长（秒）。 */
    duration: number;
    /** 当前播放位置（秒）。 */
    position: number;
    /** 是否正在播放。 */
    playing?: boolean;
    /** 点击波形时回调（秒）。 */
    onseek?: (seconds: number) => void;
    /** 画布高度。 */
    height?: number;
    /** 主题标识；变化时重新读取配色并重绘。 */
    theme?: string;
  }

  let {
    peaks,
    duration,
    position,
    playing = false,
    onseek,
    height = 96,
    theme = ""
  }: Props = $props();

  let canvas = $state<HTMLCanvasElement | null>(null);
  let width = $state(600);
  /** 当前主题下的配色（Canvas 不能直接用 CSS 变量）。 */
  let colors = $state(waveformTheme());

  /**
   * 静态波形层缓存。
   *
   * 之前每次位置更新都会重设画布尺寸（`canvas.width = …`，这会清空位图并重新
   * 分配显存）再重画上千个矩形；5 Hz 下明显掉帧，也拖慢了整页交互。
   * 现在把「峰值条」渲染到一张离屏画布，只在尺寸 / 数据 / 主题变化时重建；
   * 每帧只做一次 drawImage + 已播放区域叠加 + 播放头。
   */
  let staticLayer: HTMLCanvasElement | null = null;
  let staticKey = "";

  /** 播放进度比例 0-1。 */
  const progress = $derived(duration > 0 ? Math.min(position / duration, 1) : 0);

  /** 静态层缓存键：任一输入变化都必须重建。 */
  function staticLayerKey(): string {
    return `${width}|${height}|${peaks.length}|${colors.background}|${colors.idle}|${colors.grid}`;
  }

  /** 构建静态波形层（离屏画布）。 */
  function buildStaticLayer(): HTMLCanvasElement | null {
    const dpr = window.devicePixelRatio || 1;
    const off = document.createElement("canvas");
    off.width = Math.max(1, Math.floor(width * dpr));
    off.height = Math.max(1, Math.floor(height * dpr));
    const ctx = off.getContext("2d");
    if (!ctx) return null;

    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = colors.background;
    ctx.fillRect(0, 0, width, height);
    if (peaks.length === 0) return off;

    // 中线
    const mid = height / 2;
    ctx.strokeStyle = colors.grid;
    ctx.beginPath();
    ctx.moveTo(0, mid);
    ctx.lineTo(width, mid);
    ctx.stroke();

    // 峰值条统一用「未播放」配色，已播放部分每帧叠加
    const step = width / peaks.length;
    const barWidth = Math.max(step, 1);
    ctx.fillStyle = colors.idle;
    for (let i = 0; i < peaks.length; i++) {
      const amp = (peaks[i] / 255) * (height / 2 - 2);
      ctx.fillRect(i * step, mid - amp, barWidth, amp * 2 || 1);
    }
    return off;
  }

  /** 重绘：静态层（按需重建）+ 已播放进度 + 播放头。 */
  function draw(): void {
    const c = canvas;
    if (!c) return;
    const ctx = c.getContext("2d");
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.floor(width * dpr));
    const h = Math.max(1, Math.floor(height * dpr));

    // 只在尺寸真的变化时重设画布：重设会清空位图并重新分配
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    if (peaks.length === 0) {
      ctx.fillStyle = colors.background;
      ctx.fillRect(0, 0, width, height);
      ctx.fillStyle = colors.muted;
      ctx.font = "12px system-ui, sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText("暂无波形（后台分析完成后显示）", width / 2, height / 2);
      return;
    }

    const key = staticLayerKey();
    if (key !== staticKey || !staticLayer) {
      staticLayer = buildStaticLayer();
      staticKey = key;
    }
    if (staticLayer) ctx.drawImage(staticLayer, 0, 0, width, height);

    const playedX = progress * width;
    const mid = height / 2;

    // 已播放区域：裁剪 + 单次填充，不逐条重画
    if (playedX > 0) {
      ctx.save();
      ctx.beginPath();
      ctx.rect(0, 0, playedX, height);
      ctx.clip();
      const step = width / peaks.length;
      const barWidth = Math.max(step, 1);
      ctx.fillStyle = colors.played;
      for (let i = 0; i < peaks.length; i++) {
        const x = i * step;
        if (x > playedX) break;
        const amp = (peaks[i] / 255) * (height / 2 - 2);
        ctx.fillRect(x, mid - amp, barWidth, amp * 2 || 1);
      }
      ctx.restore();
    }

    // 播放头
    if (duration > 0) {
      ctx.strokeStyle = colors.head;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(playedX, 0);
      ctx.lineTo(playedX, height);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
  }

  /** 把点击位置换算为秒数。 */
  function handleClick(event: MouseEvent): void {
    if (!onseek || duration <= 0) return;
    const c = canvas;
    if (!c) return;
    const rect = c.getBoundingClientRect();
    const ratio = Math.min(Math.max((event.clientX - rect.left) / rect.width, 0), 1);
    onseek(ratio * duration);
  }

  // 依赖变化时重绘；主题变化时先重新读取配色
  $effect(() => {
    // 显式读取依赖，确保 $effect 会因它们变化而重新执行
    void theme;
    void peaks;
    void progress;
    void width;
    void height;
    colors = waveformTheme();
    draw();
  });

  // 监听容器尺寸
  $effect(() => {
    const c = canvas;
    if (!c?.parentElement) return;
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        width = Math.max(120, entry.contentRect.width);
      }
    });
    observer.observe(c.parentElement);
    width = Math.max(120, c.parentElement.clientWidth);
    return () => observer.disconnect();
  });
</script>

<div class="waveform" style="height: {height}px">
  <canvas
    bind:this={canvas}
    style="width: 100%; height: {height}px"
    class:clickable={!!onseek}
    class:playing
    onclick={handleClick}
    role="slider"
    aria-label="波形，点击可跳转播放位置"
    aria-valuemin="0"
    aria-valuemax={duration}
    aria-valuenow={position}
    tabindex="0"
  ></canvas>
</div>

<style>
  .waveform {
    width: 100%;
    border-radius: 8px;
    overflow: hidden;
    border: 1px solid var(--border);
    background: var(--wave-bg);
  }

  canvas {
    display: block;
  }

  canvas.clickable {
    cursor: crosshair;
  }

  canvas:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
