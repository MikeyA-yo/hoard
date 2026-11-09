import { useEffect, useRef } from "react";
import { useHoard } from "../store";
import { formatBytes, formatCount, formatDate, formatPercent } from "../lib/format";
import { CATEGORY_COLORS } from "../lib/colors";
import { sizeOf, type NodeInfo, type SortKey } from "../types";

const COLUMNS: { key: SortKey; label: string; className: string }[] = [
  { key: "name", label: "Name", className: "col-name" },
  { key: "size", label: "Size", className: "col-size" },
  { key: "count", label: "Files", className: "col-count" },
  { key: "modified", label: "Modified", className: "col-date" },
];

function kindLabel(n: NodeInfo): string {
  if (n.kind === "dir") return "Folder";
  if (n.kind === "symlink") return "Link (not followed)";
  return n.category.replace("_", " ");
}

export function ChildList() {
  const detail = useHoard((s) => s.detail);
  const children = useHoard((s) => s.children);
  const total = useHoard((s) => s.childrenTotal);
  const selectedId = useHoard((s) => s.selectedId);
  const sort = useHoard((s) => s.sort);
  const mode = useHoard((s) => s.mode);
  const select = useHoard((s) => s.select);
  const navigate = useHoard((s) => s.navigate);
  const setSort = useHoard((s) => s.setSort);
  const loadMore = useHoard((s) => s.loadMoreChildren);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const trayItems = useHoard((s) => s.tray?.items);
  const trayIds = new Set(trayItems?.map((i) => i.node.id));
  const selectedRef = useRef<HTMLTableRowElement>(null);

  useEffect(() => {
    selectedRef.current?.scrollIntoView({ block: "nearest" });
  }, [selectedId]);

  if (!detail) return <div className="placeholder">Loading…</div>;
  const parentSize = sizeOf(detail.node, mode);

  if (children.length === 0) {
    return <div className="placeholder">{detail.node.kind === "dir" ? "This folder is empty or could not be read." : "Not a folder."}</div>;
  }

  return (
    <div className="child-list">
      <table>
        <thead>
          <tr>
            {COLUMNS.map((c) => (
              <th key={c.key} className={c.className}>
                <button className={sort === c.key ? "sort active" : "sort"} onClick={() => setSort(c.key)}>
                  {c.label}
                  {sort === c.key ? (c.key === "name" ? " ↑" : " ↓") : ""}
                </button>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {children.map((n) => {
            const size = sizeOf(n, mode);
            const pct = parentSize > 0 ? (size / parentSize) * 100 : 0;
            const selected = n.id === selectedId;
            return (
              <tr
                key={n.id}
                ref={selected ? selectedRef : undefined}
                className={selected ? "selected" : undefined}
                onClick={() => select(n.id)}
                onDoubleClick={() => n.kind === "dir" && void navigate(n.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  select(n.id);
                  openContextMenu({ x: e.clientX, y: e.clientY, id: n.id, isDir: n.kind === "dir" });
                }}
                aria-selected={selected}
              >
                <td className="col-name">
                  <i className="swatch" style={{ background: CATEGORY_COLORS[n.category] }} aria-hidden="true" />
                  <span className="name" title={`${n.name} (${kindLabel(n)})`}>
                    {n.name}
                    {n.kind === "dir" ? "/" : ""}
                  </span>
                  {trayIds.has(n.id) && <span className="tray-badge">in tray</span>}
                  {n.flags.length > 0 && <span className="flag">{n.flags.join(", ").replace("_", " ")}</span>}
                </td>
                <td className="col-size">
                  <div className="size-cell">
                    <span>{formatBytes(size)}</span>
                    <span className="pct">{formatPercent(size, parentSize)}</span>
                  </div>
                  <div className="pct-bar" aria-hidden="true">
                    <div style={{ width: `${pct}%` }} />
                  </div>
                </td>
                <td className="col-count">{n.kind === "dir" ? formatCount(n.fileCount) : ""}</td>
                <td className="col-date">{formatDate(n.mtime)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {children.length < total && (
        <button className="load-more" onClick={() => void loadMore()}>
          Show more ({formatCount(total - children.length)} remaining)
        </button>
      )}
    </div>
  );
}
