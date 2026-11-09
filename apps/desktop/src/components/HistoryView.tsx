// History (PRD H1-H3): snapshots saved after each complete scan, and what
// changed between them.

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatAge, formatBytes, formatDate } from "../lib/format";
import type { DiffEntry, History } from "../types";

function signed(n: number): string {
  return n < 0 ? `-${formatBytes(-n)}` : `+${formatBytes(n)}`;
}

function label(path: string): string {
  return path === "" ? "(the scanned folder)" : path;
}

function useHistory(): History | null {
  const summary = useHoard((s) => s.summary);
  const [history, setHistory] = useState<History | null>(null);
  useEffect(() => {
    let stale = false;
    api
      .getHistory()
      .then((h) => {
        if (!stale) setHistory(h);
      })
      .catch(() => {
        if (!stale) setHistory(null);
      });
    return () => {
      stale = true;
    };
  }, [summary]);
  return history;
}

/** "Since last scan: +12.4 GB" with the top growers (PRD H3). */
export function GrowthBanner() {
  const history = useHistory();
  const setMainView = useHoard((s) => s.setMainView);
  const [hidden, setHidden] = useState(false);
  const g = history?.growth;
  if (!g || hidden || g.change === 0) return null;
  return (
    <div className="growth-banner" role="status">
      <span>
        Since the last scan ({formatAge(g.since)}): <strong>{signed(g.change)}</strong>
      </span>
      {g.top.length > 0 && (
        <span className="muted">
          {" "}
          Grew most: {g.top.map((t) => `${label(t.path)} (${signed(t.change)})`).join(", ")}
        </span>
      )}
      <button className="link" onClick={() => setMainView("history")}>
        Details
      </button>
      <button className="ghost" aria-label="Hide" onClick={() => setHidden(true)}>
        ✕
      </button>
    </div>
  );
}

export function HistoryView() {
  const history = useHistory();
  const mode = useHoard((s) => s.mode);
  const [older, setOlder] = useState<number | null>(null);
  const [newer, setNewer] = useState<number | null>(null);
  const [diff, setDiff] = useState<DiffEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const snaps = history?.snapshots ?? [];
  const a = older ?? snaps[1]?.takenAt ?? null;
  const b = newer ?? snaps[0]?.takenAt ?? null;

  useEffect(() => {
    if (a === null || b === null) return;
    let stale = false;
    api
      .diffSnapshots(a, b, mode, 500)
      .then((d) => {
        if (stale) return;
        setDiff(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [a, b, mode]);

  if (!history) return <div className="placeholder">Loading history…</div>;
  if (snaps.length < 2) {
    return (
      <div className="placeholder">
        History starts with this scan. Hoard saves a small snapshot after every complete scan; scan this folder again
        later to see what grew, shrank, appeared or disappeared.
      </div>
    );
  }

  const option = (t: number) => (
    <option key={t} value={t}>
      {formatDate(t)} ({formatAge(t)})
    </option>
  );

  return (
    <div className="suggestions">
      <header className="sugg-header">
        <div className="history-pick">
          Compare{" "}
          <select value={a ?? ""} onChange={(e) => setOlder(Number(e.target.value))}>
            {snaps.map((s) => option(s.takenAt))}
          </select>{" "}
          with{" "}
          <select value={b ?? ""} onChange={(e) => setNewer(Number(e.target.value))}>
            {snaps.map((s) => option(s.takenAt))}
          </select>
        </div>
        <span className="muted">{snaps.length} snapshots saved</span>
      </header>
      {error && <div className="banner error">{error}</div>}
      {diff && diff.length === 0 && <div className="placeholder">No folder changed between these two scans.</div>}
      {diff && diff.length > 0 && (
        <ul className="sugg-items diff-list">
          {diff.map((d) => (
            <li key={d.path}>
              <span className={`change-kind ${d.kind}`}>{d.kind}</span>
              <span className={d.change >= 0 ? "size grew" : "size shrank"}>{signed(d.change)}</span>
              <span className="mono path" title={label(d.path)}>
                {label(d.path)}
              </span>
              <span className="muted age">
                {d.before !== null ? formatBytes(d.before) : "-"} → {d.after !== null ? formatBytes(d.after) : "-"}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
