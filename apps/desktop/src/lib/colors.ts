// Treemap colors. Colors are never the only signal: blocks carry text labels
// and every view shows sizes and names as text.

import type { Category } from "../types";

export const CATEGORY_COLORS: Record<Category, string> = {
  folder: "#4a5367",
  video: "#e8615d",
  image: "#f2a43a",
  audio: "#b47be6",
  archive: "#d6c24a",
  disk_image: "#e57fb6",
  document: "#4f9cf0",
  code: "#4cc38a",
  executable: "#4dc4d2",
  other: "#8b94a7",
};

export const CATEGORY_LABELS: Record<Category, string> = {
  folder: "Folders",
  video: "Video",
  image: "Images",
  audio: "Audio",
  archive: "Archives",
  disk_image: "Disk images",
  document: "Documents",
  code: "Code",
  executable: "Apps & libraries",
  other: "Other",
};

export const MERGED_COLOR = "#5a6170";

export interface AgeBucket {
  maxDays: number;
  label: string;
  color: string;
}

/** Fresh (cool) to old (warm). */
export const AGE_BUCKETS: AgeBucket[] = [
  { maxDays: 30, label: "Under 1 month", color: "#4f9cf0" },
  { maxDays: 180, label: "1 to 6 months", color: "#4cc38a" },
  { maxDays: 365, label: "6 to 12 months", color: "#d6c24a" },
  { maxDays: 365 * 3, label: "1 to 3 years", color: "#f2a43a" },
  { maxDays: Number.POSITIVE_INFINITY, label: "Over 3 years", color: "#e8615d" },
];

export function ageColor(mtimeSecs: number, nowMs: number = Date.now()): string {
  if (!mtimeSecs) return MERGED_COLOR;
  const days = (nowMs / 1000 - mtimeSecs) / 86_400;
  const bucket = AGE_BUCKETS.find((b) => days < b.maxDays) ?? AGE_BUCKETS[AGE_BUCKETS.length - 1];
  return bucket ? bucket.color : MERGED_COLOR;
}

/** Black or white, whichever reads better on `hex`. */
export function textColorOn(hex: string): string {
  const v = parseInt(hex.slice(1), 16);
  const r = (v >> 16) & 255;
  const g = (v >> 8) & 255;
  const b = v & 255;
  const luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return luminance > 0.55 ? "#111318" : "#f4f6fa";
}

/** Lightens (amount > 0) or darkens (amount < 0) a hex color. */
export function shade(hex: string, amount: number): string {
  const v = parseInt(hex.slice(1), 16);
  const f = (c: number) => Math.max(0, Math.min(255, Math.round(c + (amount > 0 ? (255 - c) : c) * amount)));
  const r = f((v >> 16) & 255);
  const g = f((v >> 8) & 255);
  const b = f(v & 255);
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}
