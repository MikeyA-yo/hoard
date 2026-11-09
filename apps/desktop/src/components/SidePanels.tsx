import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatDate } from "../lib/format";
import { CATEGORY_COLORS } from "../lib/colors";
import { sizeOf, type ErrorsPage, type LargestItem, type LargestKind } from "../types";

const LARGEST_LIMIT = 200;

export function LargestPanel({ kind }: { kind: LargestKind }) {
  const mode = useHoard((s) => s.mode);
  const summary = useHoard((s) => s.summary);
  const navigate = useHoard((s) => s.navigate);
  const select = useHoard((s) => s.select);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const [items, setItems] = useState<LargestItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    api
      .getLargest(kind, LARGEST_LIMIT, mode)
      .then((r) => {
        if (stale) return;
        setItems(r);
        setError(null);
      })
      .catch((e: unknown) => !stale && setError(errorMessage(e)));
    return () => {
      stale = true;
    };
  }, [kind, mode, summary]);

  if (error) return <div className="placeholder">{error}</div>;
  if (!items) return <div className="placeholder">Loading…</div>;
  if (items.length === 0) return <div className="placeholder">Nothing here.</div>;

  const show = async (item: LargestItem) => {
    if (item.node.kind === "dir") {
      await navigate(item.node.id);
    } else if (item.node.parent !== null) {
      await navigate(item.node.parent);
      select(item.node.id);
    }
  };

  return (
    <ul className="largest">
      {items.map((it) => (
        <li
          key={it.node.id}
          onClick={() => void show(it)}
          onContextMenu={(e) => {
            e.preventDefault();
            openContextMenu({ x: e.clientX, y: e.clientY, id: it.node.id, isDir: it.node.kind === "dir" });
          }}
          title={it.path}
        >
          <div className="largest-top">
            <i className="swatch" style={{ background: CATEGORY_COLORS[it.node.category] }} aria-hidden="true" />
            <span className="name">{it.node.name}</span>
            <span className="size">{formatBytes(sizeOf(it.node, mode))}</span>
          </div>
          <div className="largest-path mono muted">
            {it.path}
            {it.node.mtime > 0 && <span> · {formatDate(it.node.mtime)}</span>}
          </div>
        </li>
      ))}
    </ul>
  );
}

export function ErrorsPanel() {
  const summary = useHoard((s) => s.summary);
  const [page, setPage] = useState<ErrorsPage | null>(null);

  useEffect(() => {
    let stale = false;
    api
      .getErrors(0, 1000)
      .then((p) => !stale && setPage(p))
      .catch(() => !stale && setPage({ total: 0, items: [] }));
    return () => {
      stale = true;
    };
  }, [summary]);

  if (!page) return <div className="placeholder">Loading…</div>;
  if (page.total === 0) return <div className="placeholder">No errors. Everything under this folder was readable.</div>;

  return (
    <div className="errors">
      <p className="muted">
        These locations could not be read, so the sizes of what is inside them are unknown. This is usually a
        permissions issue.
      </p>
      <ul>
        {page.items.map((e, i) => (
          <li key={`${e.path}-${i}`}>
            <div className="mono">{e.path}</div>
            <div className="muted">{e.message}</div>
          </li>
        ))}
      </ul>
      {page.total > page.items.length && <p className="muted">…and {page.total - page.items.length} more.</p>}
    </div>
  );
}
