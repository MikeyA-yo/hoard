import { useMemo } from "react";
import { useHoard } from "../store";
import { formatBytes, formatCount, formatDuration } from "../lib/format";
import { sizeOf, type TreemapData } from "../types";
import { Treemap } from "./Treemap";

export function ScanningScreen() {
  const scanPath = useHoard((s) => s.scanPath);
  const progress = useHoard((s) => s.progress);
  const batch = useHoard((s) => s.batch);
  const cancelScan = useHoard((s) => s.cancelScan);
  const scanId = useHoard((s) => s.scanId);

  // Partial treemap from the running top-level totals.
  const live = useMemo<TreemapData | null>(() => {
    if (batch.length === 0) return null;
    const children: TreemapData[] = batch.map((b) => ({
      id: b.id,
      name: b.name,
      kind: b.kind,
      category: b.category,
      size: sizeOf(b, "allocated"),
      fileCount: b.fileCount,
      mtime: 0,
      mergedCount: 0,
      hasMore: false,
      children: [],
    }));
    return {
      id: null,
      name: scanPath ?? "",
      kind: "dir",
      category: "folder",
      size: children.reduce((s, c) => s + c.size, 0),
      fileCount: 0,
      mtime: 0,
      mergedCount: 0,
      hasMore: false,
      children,
    };
  }, [batch, scanPath]);

  return (
    <div className="scanning">
      <header className="scan-header">
        <div>
          <div className="muted">Scanning</div>
          <h2 className="mono">{scanPath}</h2>
        </div>
        <button className="danger-outline" onClick={() => void cancelScan()} disabled={scanId === null}>
          Stop scan
        </button>
      </header>

      <dl className="stats">
        <div>
          <dt>Files</dt>
          <dd>{formatCount(progress?.files ?? 0)}</dd>
        </div>
        <div>
          <dt>Folders</dt>
          <dd>{formatCount(progress?.dirs ?? 0)}</dd>
        </div>
        <div>
          <dt>Counted</dt>
          <dd>{formatBytes(progress?.bytes ?? 0)}</dd>
        </div>
        <div>
          <dt>Unreadable</dt>
          <dd>{formatCount(progress?.errors ?? 0)}</dd>
        </div>
        <div>
          <dt>Elapsed</dt>
          <dd>{formatDuration(progress?.elapsedMs ?? 0)}</dd>
        </div>
      </dl>

      <div className="current-path mono" title={progress?.currentPath}>
        {progress?.currentPath ?? "Starting…"}
      </div>

      <div className="scan-live">
        {live ? <Treemap data={live} colorBy="category" interactive={false} /> : <div className="placeholder">Waiting for the first results…</div>}
      </div>
    </div>
  );
}
