/**
 * 展示层格式化工具。纯函数，便于复用与测试。
 */

import type { Tag, TagSource } from "./types";

/** 把秒数格式化为 `m:ss.mmm`（试听素材需要毫秒精度）。 */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return "--:--";
  }
  const totalMs = Math.round(seconds * 1000);
  const m = Math.floor(totalMs / 60000);
  const s = Math.floor((totalMs % 60000) / 1000);
  const ms = totalMs % 1000;

  // 一分钟以内显示毫秒，音效素材常需要精确到毫秒
  if (m === 0) {
    return `${s}.${String(ms).padStart(3, "0")}s`;
  }
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** 把秒数格式化为播放器用的 `m:ss`。 */
export function formatClock(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return "0:00";
  }
  const total = Math.floor(seconds);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

/** 人类可读的文件大小。 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || bytes <= 0) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return `${value.toFixed(value < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
}

/** 人类可读的采样率，例如 `48.0 kHz`。 */
export function formatSampleRate(hz: number | null | undefined): string {
  if (!hz) return "—";
  return `${(hz / 1000).toFixed(1)} kHz`;
}

/** 位深 / 声道描述。 */
export function formatChannels(channels: number | null | undefined): string {
  if (!channels) return "—";
  if (channels === 1) return "单声道";
  if (channels === 2) return "立体声";
  return `${channels} 声道`;
}

/** 相对时间（导入时间列）。 */
export function formatRelativeTime(unixSeconds: number | null | undefined): string {
  if (!unixSeconds) return "—";
  const now = Date.now() / 1000;
  const diff = now - unixSeconds;
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 604800) return `${Math.floor(diff / 86400)} 天前`;
  return new Date(unixSeconds * 1000).toLocaleDateString("zh-CN");
}

/** 标签来源的中文名。 */
export function sourceLabel(source: TagSource): string {
  const map: Record<TagSource, string> = {
    user: "用户",
    yamnet: "YAMNet",
    rule: "规则",
    inherited: "继承",
    clap: "CLAP"
  };
  return map[source] ?? source;
}

/**
 * 把标签文本按当前词汇表翻译成展示名。
 *
 * 只有规则引擎产出的**声学描述**才翻译；用户标签与模型类别标签保持原样
 * （与后端 `tag_vocab::is_translatable` 的规则一致）。
 */
export function displayTag(tag: Tag, table: Record<string, string>): string {
  if (tag.source !== "rule") return tag.tag;
  return table[tag.tag] ?? tag.tag;
}

/**
 * 标签的视觉分类。对应策划书 7.2：
 * 「自动标签灰色显示，带置信度；用户确认后变为彩色」。
 */
export type TagKind = "user" | "confirmed" | "auto" | "inherited" | "low";

/** 判断标签应如何着色。 */
export function tagKind(tag: Tag): TagKind {
  if (tag.source === "user") return "user";
  if (tag.source === "inherited") return "inherited";
  // 自动标签被用户确认过，或来源本身已是高置信度
  if (tag.is_confirmed) return "confirmed";
  if (tag.confidence < 0.5) return "low";
  return "auto";
}

/** 置信度百分比文本。 */
export function confidenceText(tag: Tag): string {
  if (tag.source === "user") return "用户添加";
  return `${Math.round(tag.confidence * 100)}%`;
}

/** 取路径的最后一段（显示用）。 */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** 取父目录（显示用）。 */
export function dirName(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return idx > 0 ? path.slice(0, idx) : "";
}
