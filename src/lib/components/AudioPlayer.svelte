<script lang="ts">
  /**
   * 播放器控制条 + 波形（策划书 7.1 底部区域）。
   *
   * 波形数据来自后端 `get_waveform`（固定 1024 个峰值），播放状态由
   * `playback_state` 轮询获得。
   *
   * 流畅度设计：
   * * **本地外推 + rAF**：两次轮询之间用本地时钟推算位置、每帧刷新，
   *   播放头连续移动，不受轮询间隔限制；
   * * **自适应轮询**：播放中 120ms、空闲 500ms，兼顾跟手与开销；
   * * **乐观更新**：播放/暂停按钮先本地翻转状态再等后端确认，
   *   按下即有反馈（见 `library.togglePlayPause`）。
   */
  import { onMount, untrack } from "svelte";
  import Waveform from "./Waveform.svelte";
  import { library } from "../stores.svelte";
  import * as api from "../api";
  import { formatClock, formatSampleRate, formatChannels, formatBytes } from "../format";
  import type { AudioItem } from "../types";

  interface Props {
    /** 当前聚焦条目。 */
    item: AudioItem | null;
  }

  let { item }: Props = $props();

  /** 波形峰值缓存。 */
  let peaks = $state<number[]>([]);
  /** 正在加载波形。 */
  let loadingWave = $state(false);
  /** 已加载波形的音频 id，避免重复请求。 */
  let loadedFor = $state<number | null>(null);

  /** 轮询与动画句柄。 */
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let rafId: number | null = null;

  /** 平滑显示位置（秒）。 */
  let smoothPosition = $state(0);
  /** 上次同步时刻（performance.now，毫秒）与该时刻的位置，用于外推。 */
  let anchorAt = 0;
  let anchorPos = 0;

  /** 轮询间隔：播放中密、空闲时疏。 */
  const POLL_PLAYING_MS = 120;
  const POLL_IDLE_MS = 500;

  onMount(() => {
    /** 把平滑位置对齐到后端上报值。 */
    const syncSmooth = (pos: number, playing: boolean): void => {
      anchorPos = pos;
      anchorAt = performance.now();
      smoothPosition = pos;
      void playing;
    };

    const poll = async () => {
      const st0 = library.playback;
      if (st0.audio_id !== null) {
        await library.syncPlayback();
        const st = library.playback;
        if (!st.playing || Math.abs(st.position - smoothPosition) > 0.15) {
          // 状态切换或偏差较大：直接对齐
          syncSmooth(st.position, st.playing);
        } else {
          // 偏差很小：只重置外推基准，避免画面抖动
          anchorPos = st.position;
          anchorAt = performance.now();
        }
      }
      pollTimer = setTimeout(poll, library.playback.playing ? POLL_PLAYING_MS : POLL_IDLE_MS);
    };
    pollTimer = setTimeout(poll, POLL_PLAYING_MS);

    const tick = () => {
      const st = library.playback;
      if (st.audio_id === null) {
        smoothPosition = 0;
      } else if (st.playing) {
        const elapsed = (performance.now() - anchorAt) / 1000;
        const next = anchorPos + elapsed;
        smoothPosition = st.duration > 0 ? Math.min(next, st.duration) : next;
      } else {
        // 暂停 / 播完：直接跟随后端
        smoothPosition = st.position;
      }
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);

    return () => {
      if (pollTimer !== null) clearTimeout(pollTimer);
      if (rafId !== null) cancelAnimationFrame(rafId);
    };
  });

  // 聚焦条目变化时加载波形
  $effect(() => {
    const id = item?.id ?? null;
    if (id === null || id === loadedFor) return;
    let cancelled = false;
    loadingWave = true;
    void (async () => {
      try {
        const data = await api.getWaveform(id);
        if (!cancelled) {
          peaks = data;
          loadedFor = id;
        }
      } catch {
        if (!cancelled) peaks = [];
      } finally {
        if (!cancelled) loadingWave = false;
      }
    })();
    return () => {
      cancelled = true;
    };
  });

  // 切换到别的音频时把播放头归零
  $effect(() => {
    const currentId = library.playback.audio_id;
    untrack(() => {
      if ((item?.id ?? null) !== currentId) {
        smoothPosition = 0;
        anchorPos = 0;
        anchorAt = performance.now();
      }
    });
  });

  /** 播放进度是否属于当前条目。 */
  const isCurrent = $derived(item !== null && library.playback.audio_id === item.id);
  /** 显示用位置：当前条目用平滑值，否则归零。 */
  const position = $derived(isCurrent ? smoothPosition : 0);
  const duration = $derived(item?.duration ?? 0);

  /** 键盘快捷键：空格播放/暂停。 */
  function handleKey(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    // 输入框里不拦截
    if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
    if (event.code === "Space" && item) {
      event.preventDefault();
      if (isCurrent) void library.togglePlayPause();
      else void library.play(item.id);
    }
  }
</script>

<svelte:window onkeydown={handleKey} />

<div class="player">
  {#if item}
    <div class="top">
      <div class="info">
        <span class="name" title={item.path}>{item.filename}</span>
        <span class="meta">
          {formatSampleRate(item.sample_rate)}
          · {formatChannels(item.channels)}
          · {formatBytes(item.file_size)}
          {#if item.format}
            · {item.format.toUpperCase()}
          {/if}
        </span>
      </div>

      <div class="controls">
        <button
          class="btn"
          onclick={() => library.playNext(-1)}
          title="上一个"
          aria-label="上一个"
        >⏮</button>

        <button
          class="btn primary"
          onclick={() => {
            if (isCurrent) void library.togglePlayPause();
            else void library.play(item.id);
          }}
          title={isCurrent && library.playback.playing ? "暂停 (空格)" : "播放 (空格)"}
          aria-label={isCurrent && library.playback.playing ? "暂停" : "播放"}
        >
          {isCurrent && library.playback.playing ? "❚❚" : "▶"}
        </button>

        <button class="btn" onclick={() => library.stop()} title="停止" aria-label="停止">⏹</button>
        <button class="btn" onclick={() => library.playNext(1)} title="下一个" aria-label="下一个">⏭</button>

        <span class="time">{formatClock(position)} / {formatClock(duration)}</span>

        <label class="volume" title="音量">
          <span aria-hidden="true">🔊</span>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={library.playback.volume}
            oninput={(e) =>
              library.changeVolume(Number((e.currentTarget as HTMLInputElement).value))}
            aria-label="音量"
          />
        </label>
      </div>
    </div>

    <div class="wave">
      {#if loadingWave}
        <div class="wave-placeholder">正在加载波形…</div>
      {:else}
        <Waveform
          {peaks}
          {duration}
          {position}
          playing={isCurrent && library.playback.playing}
          theme={library.theme}
          onseek={(s) => {
            if (isCurrent) void library.seek(s);
            else void library.play(item.id).then(() => library.seek(s));
          }}
        />
      {/if}
    </div>
  {:else}
    <div class="idle">选中一个音频即可试听</div>
  {/if}
</div>

<style>
  .player {
    border-top: 1px solid var(--border);
    background: var(--panel);
    padding: 10px 14px 12px;
    flex-shrink: 0;
  }

  .top {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 9px;
  }

  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }

  .name {
    font-size: 13px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 2px;
  }

  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .btn {
    width: 30px;
    height: 30px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--chip-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 11px;
    padding: 0;
  }

  .btn:hover {
    color: var(--text);
    border-color: var(--accent);
  }

  .btn.primary {
    width: 36px;
    height: 36px;
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-contrast);
    font-size: 13px;
  }

  .btn.primary:hover {
    filter: brightness(1.1);
    color: var(--accent-contrast);
  }

  .time {
    font-size: 12px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    min-width: 84px;
    text-align: center;
  }

  .volume {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }

  .volume input {
    width: 92px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .wave {
    width: 100%;
  }

  .wave-placeholder {
    height: 96px;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--wave-bg);
    color: var(--text-dim);
    font-size: 12px;
  }

  .idle {
    height: 60px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-dim);
    font-size: 13px;
  }
</style>
