/**
 * 主题系统。
 *
 * 设计约束：
 * * **低饱和度**：所有主题的强调色与状态色都控制在较低饱和度，
 *   长时间盯着素材库不累眼，也不会和音频软件常见的深色界面冲突。
 * * **单一来源**：组件只引用 CSS 变量（见 `app.css`），主题切换仅替换变量值，
 *   组件代码零改动。
 * * **持久化**：主题标识存进后端 `settings` 表，重启后保持。
 */

/** 主题标识（必须与后端 `utils/config.rs` 的 `THEMES` 一致）。 */
export type ThemeId = "graphite" | "midnight" | "forest" | "dawn" | "sand" | "mint";

/** 主题元信息。 */
export interface ThemeMeta {
  id: ThemeId;
  /** 展示名。 */
  label: string;
  /** 深色 / 浅色。 */
  scheme: "dark" | "light";
  /** 一句话说明，用于设置面板。 */
  hint: string;
  /** 主题色板的代表色（仅用于选择器上的预览小圆点）。 */
  swatch: [string, string, string];
}

/** 内置主题清单。 */
export const THEMES: ThemeMeta[] = [
  {
    id: "graphite",
    label: "石墨",
    scheme: "dark",
    hint: "中性深灰，最接近专业音频软件，最耐看",
    swatch: ["#14171c", "#1e232b", "#93a4b8"]
  },
  {
    id: "midnight",
    label: "靛蓝",
    scheme: "dark",
    hint: "偏蓝的深色，冷调、安静",
    swatch: ["#12161f", "#1b212e", "#8fa8cf"]
  },
  {
    id: "forest",
    label: "松绿",
    scheme: "dark",
    hint: "带绿意的深色，低饱和、清爽",
    swatch: ["#131a17", "#1c2621", "#9dbfa8"]
  },
  {
    id: "dawn",
    label: "晨雾",
    scheme: "light",
    hint: "冷白底 + 雾蓝强调色，明亮但不刺眼",
    swatch: ["#f6f8fa", "#ffffff", "#5b8bb0"]
  },
  {
    id: "sand",
    label: "暖沙",
    scheme: "light",
    hint: "米色底 + 陶土强调色，柔和护眼",
    swatch: ["#faf7f2", "#fffdfa", "#b08361"]
  },
  {
    id: "mint",
    label: "薄荷",
    scheme: "light",
    hint: "浅绿底 + 青绿强调色，最清新",
    swatch: ["#f3f8f6", "#ffffff", "#5f9b87"]
  }
];

/** 默认主题。 */
export const DEFAULT_THEME: ThemeId = "graphite";

/** 主题标识 -> 元信息。 */
const BY_ID = new Map<string, ThemeMeta>(THEMES.map((t) => [t.id, t]));

/** 校验并规范化主题标识，未知值回退到默认主题。 */
export function normalizeTheme(id: string | null | undefined): ThemeId {
  return id && BY_ID.has(id) ? (id as ThemeId) : DEFAULT_THEME;
}

/** 取主题元信息。 */
export function themeMeta(id: string): ThemeMeta {
  return BY_ID.get(id) ?? BY_ID.get(DEFAULT_THEME)!;
}

/**
 * 应用主题：设置根元素属性。
 *
 * 颜色本身全部由 `app.css` 里的 `[data-theme=...]` 规则提供，
 * 这里只负责切换标识与 `color-scheme`（影响滚动条、表单控件等原生外观）。
 */
export function applyTheme(id: string): void {
  const theme = normalizeTheme(id);
  const root = document.documentElement;
  root.setAttribute("data-theme", theme);
  root.style.colorScheme = themeMeta(theme).scheme;
}

/** 波形绘制所需的颜色（Canvas 无法直接使用 CSS 变量，需读取计算值）。 */
export interface WaveformTheme {
  /** 背景。 */
  background: string;
  /** 未播放部分。 */
  idle: string;
  /** 已播放部分。 */
  played: string;
  /** 播放头。 */
  head: string;
  /** 中线。 */
  grid: string;
  /** 占位文字。 */
  muted: string;
}

/**
 * 读取当前主题下波形的颜色。
 *
 * 必须在 DOM 上读取 —— 主题切换后这些值会变，因此调用方需要在
 * `data-theme` 变化时重新调用本函数并重绘。
 */
export function waveformTheme(): WaveformTheme {
  const cs = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string): string => {
    const v = cs.getPropertyValue(name).trim();
    return v || fallback;
  };
  return {
    background: read("--wave-bg", "#12161c"),
    idle: read("--wave-idle", "#3a4553"),
    played: read("--wave-played", "#7ba7cc"),
    head: read("--wave-head", "#d8c08a"),
    grid: read("--wave-grid", "#242c36"),
    muted: read("--text-dim", "#8d99a8")
  };
}
