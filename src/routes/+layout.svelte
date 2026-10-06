<script lang="ts">
  /**
   * 全局布局：注入主题与基础样式。
   *
   * 主题在最早的时机应用（`onMount` 里同步读一次持久化设置），
   * 避免先渲染默认主题再跳到用户主题的闪烁。
   */
  import { onMount } from "svelte";
  import "../app.css";
  import { applyTheme } from "$lib/theme";

  interface Props {
    children?: import("svelte").Snippet;
  }

  let { children }: Props = $props();

  onMount(async () => {
    // 动态引入，避免启动时因 api 依赖阻塞首屏
    const { getSettings } = await import("$lib/api");
    try {
      const s = await getSettings();
      applyTheme(s.theme);
    } catch {
      // 读不到设置（例如后端尚未就绪）时保持默认主题
    }
  });
</script>

{@render children?.()}
