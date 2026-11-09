// Apps lens (PRD L5): installed apps with their install folder (exact) and
// data folders matched by name (likely, added one at a time).

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatCount } from "../lib/format";
import type { AppFolder, Apps } from "../types";

const IS_MAC = navigator.userAgent.includes("Mac");
const PAGE = 100;

export function AppsView() {
  const summary = useHoard((s) => s.summary);
  const [apps, setApps] = useState<Apps | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [shown, setShown] = useState(PAGE);

  useEffect(() => {
    let stale = false;
    api
      .getApps()
      .then((a) => {
        if (stale) return;
        setApps(a);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [summary]);

  if (error) return <div className="placeholder">{error}</div>;
  if (!apps) return <div className="placeholder">Reading installed apps…</div>;
  if (apps.apps.length === 0) {
    return (
      <div className="placeholder">
        {apps.registered === 0
          ? "No installed apps could be read on this system yet."
          : "None of the installed apps keep files inside this folder. Scan the C: drive or your home folder to see apps."}
      </div>
    );
  }

  return (
    <div className="suggestions">
      <p className="muted small sugg-intro">
        {formatCount(apps.registered)} apps are installed; {formatCount(apps.apps.length)} have files in this scan. The
        install folder is exact; data folders are matched by name ("likely"), so check them before adding. To remove an
        app itself, uninstall it from {IS_MAC ? "Finder" : "Settings › Apps"}.
      </p>
      {apps.apps.slice(0, shown).map((a) => (
        <section key={`${a.name}-${a.install?.node.id ?? ""}`} className="sugg-card">
          <div className="sugg-top">
            <h3>{a.name}</h3>
            <span className="sugg-size">{formatBytes(a.total)}</span>
          </div>
          <p className="muted small">
            {[a.publisher, a.version && `version ${a.version}`].filter(Boolean).join(" · ")}
            {a.estimatedSize !== null && ` · registered size ${formatBytes(a.estimatedSize)}`}
          </p>
          <ul className="sugg-items">
            {a.install && <FolderRow folder={a.install} label="install" addable={false} />}
            {a.data.map((d) => (
              <FolderRow key={d.node.id} folder={d} label="data" addable />
            ))}
          </ul>
        </section>
      ))}
      {apps.apps.length > shown && (
        <button className="load-more" onClick={() => setShown(shown + PAGE)}>
          Show more ({formatCount(apps.apps.length - shown)} left)
        </button>
      )}
    </div>
  );
}

function FolderRow({ folder, label, addable }: { folder: AppFolder; label: string; addable: boolean }) {
  const trayItems = useHoard((s) => s.tray?.items);
  const addToTray = useHoard((s) => s.addToTray);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const cleaning = useHoard((s) => s.cleanup?.status === "running");
  const inTray = trayItems?.some((t) => t.node.id === folder.node.id) ?? false;
  return (
    <li
      onContextMenu={(e) => {
        e.preventDefault();
        openContextMenu({ x: e.clientX, y: e.clientY, id: folder.node.id, isDir: true });
      }}
    >
      <span className="size">{formatBytes(folder.node.sizeAllocated)}</span>
      <span className={`badge ${folder.confidence === "exact" ? "safe" : "review"}`}>
        {label}: {folder.confidence}
      </span>
      <span className="mono path" title={folder.path}>
        {folder.path}
      </span>
      {addable ? (
        inTray ? (
          <span className="tray-badge">in tray</span>
        ) : (
          <button className="small-btn" disabled={cleaning} onClick={() => void addToTray([folder.node.id])}>
            Add
          </button>
        )
      ) : (
        <span className="muted small">uninstall instead</span>
      )}
    </li>
  );
}
