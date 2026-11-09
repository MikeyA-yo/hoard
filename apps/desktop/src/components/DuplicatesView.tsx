// Duplicates lens (PRD L6): identical files, keep one copy of each.

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatCount, formatDate } from "../lib/format";
import { suggestKeeper } from "../lib/dupes";
import type { DupGroup, DupesPage } from "../types";

const MB = 1024 * 1024;
const MIN_SIZES = [
  { label: "1 MB", value: MB },
  { label: "10 MB", value: 10 * MB },
  { label: "100 MB", value: 100 * MB },
];
const PAGE = 50;

export function DuplicatesView() {
  const run = useHoard((s) => s.dupesRun);
  const version = useHoard((s) => s.dupesVersion);
  const findDuplicates = useHoard((s) => s.findDuplicates);
  const cancelDuplicates = useHoard((s) => s.cancelDuplicates);
  const cleaning = useHoard((s) => s.cleanup?.status === "running");
  const [page, setPage] = useState<DupesPage | null>(null);
  const [limit, setLimit] = useState(PAGE);
  const [minSize, setMinSize] = useState(MB);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    api
      .getDuplicates(0, limit)
      .then((p) => {
        if (!stale) setPage(p);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [version, limit]);

  if (run?.status === "running") {
    const p = run.progress;
    const pct = p && p.bytesTotal > 0 ? (p.bytesDone / p.bytesTotal) * 100 : 0;
    return (
      <div className="suggestions">
        <h3>Looking for duplicates…</h3>
        <p className="muted">
          {p
            ? `${p.stage === 1 ? "Quick check (start and end of each file)" : "Full comparison"}: ${formatCount(p.filesDone)} of ${formatCount(p.filesTotal)} files, ${formatBytes(p.bytesDone)} of ${formatBytes(p.bytesTotal)}`
            : "Grouping files by size…"}
        </p>
        <div className="usage-bar" aria-hidden="true">
          <div className="usage-fill" style={{ width: `${pct}%` }} />
        </div>
        <p>
          <button onClick={() => void cancelDuplicates()}>Stop</button>
        </p>
      </div>
    );
  }

  const searchControls = (
    <div className="dupe-controls">
      <label>
        Ignore files smaller than{" "}
        <select value={minSize} onChange={(e) => setMinSize(Number(e.target.value))}>
          {MIN_SIZES.map((m) => (
            <option key={m.value} value={m.value}>
              {m.label}
            </option>
          ))}
        </select>
      </label>
      <button className="primary" disabled={cleaning} onClick={() => void findDuplicates(minSize)}>
        {page?.ready ? "Search again" : "Find duplicates"}
      </button>
    </div>
  );

  if (error) return <div className="placeholder">{error}</div>;
  if (!page || !page.ready) {
    return (
      <div className="suggestions">
        <h3>Duplicate files</h3>
        <p className="muted">
          Hoard compares files of the same size, first by their start and end, then completely. Only exact copies are
          shown. This reads the files, so it can take a while on a big folder; you can stop it at any time.
        </p>
        {run?.status === "error" && <div className="banner error">{run.error}</div>}
        {searchControls}
      </div>
    );
  }

  return (
    <div className="suggestions">
      <header className="sugg-header">
        <div>
          <strong>{formatCount(page.totalGroups)} sets of duplicates.</strong> Keeping one copy of each would free{" "}
          <strong>{formatBytes(page.wasted)}</strong>.
          {page.unreadable > 0 && <span className="muted"> {formatCount(page.unreadable)} files could not be read.</span>}
        </div>
        {searchControls}
      </header>
      <p className="muted small sugg-intro">
        Choose the copy to keep; the others go to the Cleanup Tray. When cleaning, a copy is only removed if the one you
        kept is still there and unchanged.
      </p>
      {page.groups.map((g, i) => (
        <DupCard key={`${g.files[0]?.node.id ?? i}`} group={g} disabled={cleaning} />
      ))}
      {page.totalGroups > page.groups.length && (
        <button className="load-more" onClick={() => setLimit(limit + PAGE)}>
          Show more ({formatCount(page.totalGroups - page.groups.length)} left)
        </button>
      )}
    </div>
  );
}

function DupCard({ group, disabled }: { group: DupGroup; disabled: boolean }) {
  const keepOneCopy = useHoard((s) => s.keepOneCopy);
  const trayItems = useHoard((s) => s.tray?.items);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const [keep, setKeep] = useState<number | null>(() => suggestKeeper(group.files));
  const inTray = new Set(trayItems?.map((t) => t.node.id));
  const others = group.files.map((f) => f.node.id).filter((id) => id !== keep && !inTray.has(id));
  const name = group.files[0]?.node.name ?? "";

  return (
    <section className="sugg-card review">
      <div className="sugg-top">
        <h3>{name}</h3>
        <span className="sugg-size">{formatBytes(group.wasted)}</span>
        <span className="muted">
          {group.files.length} copies of {formatBytes(group.size)}
        </span>
      </div>
      <ul className="sugg-items">
        {group.files.map((f) => (
          <li
            key={f.node.id}
            onContextMenu={(e) => {
              e.preventDefault();
              openContextMenu({ x: e.clientX, y: e.clientY, id: f.node.id, isDir: false });
            }}
          >
            <label className="keep">
              <input
                type="radio"
                name={`keep-${group.files[0]?.node.id ?? 0}`}
                checked={keep === f.node.id}
                onChange={() => setKeep(f.node.id)}
              />
              keep
            </label>
            <span className="mono path" title={f.path}>
              {f.path}
            </span>
            <span className="muted age">{formatDate(f.node.mtime)}</span>
            {inTray.has(f.node.id) && <span className="tray-badge">in tray</span>}
          </li>
        ))}
      </ul>
      <div className="sugg-actions">
        <button
          disabled={disabled || keep === null || others.length === 0}
          onClick={() => keep !== null && void keepOneCopy(keep, others)}
        >
          Keep the selected copy, add {others.length} other{others.length === 1 ? "" : "s"} to tray
        </button>
      </div>
    </section>
  );
}
