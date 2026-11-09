// File types lens (PRD L2): totals per category; pick one to see its
// largest files.

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatCount, formatDate, formatPercent } from "../lib/format";
import { CATEGORY_COLORS, CATEGORY_LABELS } from "../lib/colors";
import { sizeOf, type Category, type CategoryTotal, type LargestItem } from "../types";

const FILES_LIMIT = 200;

export function FileTypesView() {
  const summary = useHoard((s) => s.summary);
  const mode = useHoard((s) => s.mode);
  const trayItems = useHoard((s) => s.tray?.items);
  const addToTray = useHoard((s) => s.addToTray);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const cleaning = useHoard((s) => s.cleanup?.status === "running");
  const [types, setTypes] = useState<CategoryTotal[] | null>(null);
  const [selected, setSelected] = useState<Category | null>(null);
  const [files, setFiles] = useState<LargestItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    api
      .getFileTypes()
      .then((t) => {
        if (stale) return;
        setTypes(t);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [summary]);

  useEffect(() => {
    if (selected === null) return;
    let stale = false;
    api
      .getCategoryFiles(selected, FILES_LIMIT, mode)
      .then((f) => {
        if (!stale) setFiles(f);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [selected, mode, summary]);

  if (error) return <div className="placeholder">{error}</div>;
  if (!types) return <div className="placeholder">Adding up file types…</div>;

  const sorted = [...types].sort((a, b) => sizeOf(b, mode) - sizeOf(a, mode));
  const total = sorted.reduce((s, t) => s + sizeOf(t, mode), 0);
  const inTray = new Set(trayItems?.map((i) => i.node.id));

  return (
    <div className="file-types">
      <ul className="type-list" role="listbox" aria-label="File types">
        {sorted.map((t) => {
          const size = sizeOf(t, mode);
          return (
            <li
              key={t.category}
              role="option"
              aria-selected={selected === t.category}
              className={selected === t.category ? "on" : ""}
              onClick={() => {
                setFiles(null);
                setSelected(t.category);
              }}
            >
              <div className="type-top">
                <i className="swatch" style={{ background: CATEGORY_COLORS[t.category] }} aria-hidden="true" />
                <span className="type-name">{CATEGORY_LABELS[t.category]}</span>
                <span className="sugg-size">{formatBytes(size)}</span>
              </div>
              <div className="pct-bar" aria-hidden="true">
                <div style={{ width: `${total > 0 ? (size / total) * 100 : 0}%`, background: CATEGORY_COLORS[t.category] }} />
              </div>
              <div className="muted small">
                {formatPercent(size, total)} · {formatCount(t.files)} files
              </div>
            </li>
          );
        })}
      </ul>

      <div className="type-files">
        {selected === null ? (
          <div className="placeholder">Pick a file type to see its largest files.</div>
        ) : !files ? (
          <div className="placeholder">Loading…</div>
        ) : (
          <>
            <h3>Largest {CATEGORY_LABELS[selected].toLowerCase()}</h3>
            <ul className="sugg-items">
              {files.map((f) => (
                <li
                  key={f.node.id}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    openContextMenu({ x: e.clientX, y: e.clientY, id: f.node.id, isDir: false });
                  }}
                >
                  <span className="size">{formatBytes(sizeOf(f.node, mode))}</span>
                  <span className="mono path" title={f.path}>
                    {f.path}
                  </span>
                  <span className="muted age">{formatDate(f.node.mtime)}</span>
                  {inTray.has(f.node.id) ? (
                    <span className="tray-badge">in tray</span>
                  ) : (
                    <button className="small-btn" disabled={cleaning} onClick={() => void addToTray([f.node.id])}>
                      Add
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}
