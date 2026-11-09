import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatAge, formatBytes, formatCount, joinPath } from "../lib/format";
import { minBytesFor } from "../lib/treemap";
import { AGE_BUCKETS, CATEGORY_COLORS, CATEGORY_LABELS } from "../lib/colors";
import { sizeOf, type Category, type TreemapData } from "../types";
import { Treemap, type HoverInfo } from "./Treemap";

// Deep enough to show structure under a dominant folder chain; the server
// prunes breadth-first within a node budget, so this never explodes.
const DEPTH = 5;
const LEGEND: Category[] = ["video", "image", "audio", "archive", "disk_image", "document", "code", "executable", "other"];

export function TreemapView() {
  const currentId = useHoard((s) => s.currentId);
  const detail = useHoard((s) => s.detail);
  const mode = useHoard((s) => s.mode);
  const colorBy = useHoard((s) => s.colorBy);
  const selectedId = useHoard((s) => s.selectedId);
  const select = useHoard((s) => s.select);
  const navigate = useHoard((s) => s.navigate);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const showToast = useHoard((s) => s.showToast);

  const [box, setBox] = useState({ w: 0, h: 0 });
  const [data, setData] = useState<TreemapData | null>(null);
  // Hover is tied to the treemap it came from, so a stale hover from the
  // previous folder is never combined with the new folder's path.
  const [hoverState, setHoverState] = useState<{ info: HoverInfo; data: TreemapData } | null>(null);
  const hover = hoverState && hoverState.data === data ? hoverState.info : null;

  const onResize = useCallback((w: number, h: number) => setBox({ w, h }), []);

  const nodeSize = detail && detail.node.id === currentId ? sizeOf(detail.node, mode) : null;

  useEffect(() => {
    if (currentId === null || nodeSize === null || box.w === 0 || box.h === 0) return;
    let stale = false;
    // Debounce resizes a little so dragging the window does not flood IPC.
    const t = setTimeout(() => {
      api
        .getTreemap(currentId, DEPTH, minBytesFor(nodeSize, box.w, box.h), mode)
        .then((d) => {
          if (!stale) setData(d);
        })
        .catch((e: unknown) => showToast(errorMessage(e)));
    }, 60);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [currentId, nodeSize, box, mode, showToast]);

  const onContext = useCallback(
    (x: number, y: number, id: number, isDir: boolean) => openContextMenu({ x, y, id, isDir }),
    [openContextMenu],
  );

  const hoverPath = hover && detail ? joinPath(detail.path, hover.pathParts) : null;

  return (
    <div className="treemap-view">
      <Treemap
        data={data}
        colorBy={colorBy}
        selectedId={selectedId}
        onSelect={select}
        onDrill={(id) => void navigate(id)}
        onHover={(info) => setHoverState(info && data ? { info, data } : null)}
        onContextMenu={onContext}
        onResize={onResize}
      />
      <div className="treemap-footer">
        <div className="hover-info" aria-live="polite">
          {hover ? (
            <>
              <span className="mono path">{hover.mergedCount > 0 ? hover.name : hoverPath}</span>
              <span>{formatBytes(hover.size)}</span>
              {hover.fileCount > 0 && <span className="muted">{formatCount(hover.fileCount)} files</span>}
              {hover.mtime > 0 && <span className="muted">modified {formatAge(hover.mtime)}</span>}
            </>
          ) : (
            <span className="muted">Hover a block for details. Double-click to open a folder, right-click for actions.</span>
          )}
        </div>
        <div className="legend">
          {colorBy === "category"
            ? LEGEND.map((c) => (
                <span key={c} className="legend-item">
                  <i style={{ background: CATEGORY_COLORS[c] }} />
                  {CATEGORY_LABELS[c]}
                </span>
              ))
            : AGE_BUCKETS.map((b) => (
                <span key={b.label} className="legend-item">
                  <i style={{ background: b.color }} />
                  {b.label}
                </span>
              ))}
        </div>
      </div>
    </div>
  );
}
