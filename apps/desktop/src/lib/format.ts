// Formatting helpers. Byte formatting matches hoard_core::format::bytes:
// binary (1024) multiples with short labels by default, IEC labels optional.

const SHORT = ["B", "KB", "MB", "GB", "TB", "PB", "EB"] as const;
const IEC = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] as const;

export function formatBytes(n: number, iec = false): string {
  const units = iec ? IEC : SHORT;
  if (!Number.isFinite(n) || n < 0) return "?";
  if (n < 1024) return `${Math.round(n)} ${units[0]}`;
  let value = n;
  let unit = 0;
  while ((value >= 1024 || value.toFixed(1) === "1024.0") && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

export function formatCount(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

/** Percentage of `part` in `whole`, one decimal, "0%" for an empty whole. */
export function formatPercent(part: number, whole: number): string {
  if (whole <= 0) return "0%";
  const p = (part * 100) / whole;
  if (p > 0 && p < 0.1) return "<0.1%";
  return `${p.toFixed(1)}%`;
}

/** Unix seconds to a local date like "8 Nov 2026". Empty for 0. */
export function formatDate(unixSecs: number): string {
  if (!unixSecs) return "";
  return new Date(unixSecs * 1000).toLocaleDateString("en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

/** Coarse age like "3 days ago", relative to `nowMs`. */
export function formatAge(unixSecs: number, nowMs: number = Date.now()): string {
  if (!unixSecs) return "";
  const secs = Math.max(0, nowMs / 1000 - unixSecs);
  const day = 86_400;
  if (secs < 60) return "just now";
  if (secs < 3600) return plural(Math.floor(secs / 60), "minute");
  if (secs < day) return plural(Math.floor(secs / 3600), "hour");
  if (secs < 30 * day) return plural(Math.floor(secs / day), "day");
  if (secs < 365 * day) return plural(Math.floor(secs / (30 * day)), "month");
  return plural(Math.floor(secs / (365 * day)), "year");
}

function plural(n: number, unit: string): string {
  return `${n} ${unit}${n === 1 ? "" : "s"} ago`;
}

/** Milliseconds to "4.2 s" or "2 min 05 s". */
export function formatDuration(ms: number): string {
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)} s`;
  const m = Math.floor(s / 60);
  const rest = Math.floor(s % 60);
  return `${m} min ${rest.toString().padStart(2, "0")} s`;
}

/** The path separator used by a scanned root path. */
export function separatorFor(path: string): string {
  return path.includes("\\") ? "\\" : "/";
}

/** Joins path segments onto a base using the base's separator. */
export function joinPath(base: string, parts: string[]): string {
  const sep = separatorFor(base);
  const trimmed = base.endsWith(sep) ? base.slice(0, -1) : base;
  return [trimmed, ...parts].join(sep);
}
