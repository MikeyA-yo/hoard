import type { DupFile } from "../types";

const DISPOSABLE = [/[\\/]downloads[\\/]/i, /[\\/]temp[\\/]/i, /[\\/]tmp[\\/]/i, /[\\/]\$recycle\.bin[\\/]/i];

/**
 * The copy to keep by default: one outside Downloads/Temp if possible, then
 * the oldest, then the shortest path. The user can always pick another.
 */
export function suggestKeeper(files: DupFile[]): number | null {
  if (files.length === 0) return null;
  const score = (f: DupFile) => (DISPOSABLE.some((r) => r.test(f.path)) ? 1 : 0);
  const sorted = [...files].sort(
    (a, b) => score(a) - score(b) || a.node.mtime - b.node.mtime || a.path.length - b.path.length,
  );
  return sorted[0]?.node.id ?? null;
}
