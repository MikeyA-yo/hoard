// Projects lens (PRD L3): build and dependency folders grouped by project,
// with when each project was last worked on.

import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatAge, formatBytes, formatCount } from "../lib/format";
import type { Project, Projects } from "../types";

const INACTIVE_DAYS = 90;
const PAGE = 100;

type Sort = "size" | "inactive";

function isInactive(p: Project, nowSecs: number): boolean {
  return p.lastActivity > 0 && nowSecs - p.lastActivity > INACTIVE_DAYS * 86_400;
}

export function ProjectsView() {
  const summary = useHoard((s) => s.summary);
  const trayItems = useHoard((s) => s.tray?.items);
  const addToTray = useHoard((s) => s.addToTray);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const cleaning = useHoard((s) => s.cleanup?.status === "running");
  const [data, setData] = useState<Projects | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sort, setSort] = useState<Sort>("size");
  const [shown, setShown] = useState(PAGE);
  const [nowSecs] = useState(() => Date.now() / 1000);

  // Reload whenever the tree changes (new scan or cleanup updates summary).
  useEffect(() => {
    let stale = false;
    api
      .getProjects()
      .then((d) => {
        if (stale) return;
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!stale) setError(errorMessage(e));
      });
    return () => {
      stale = true;
    };
  }, [summary]);

  const sorted = useMemo(() => {
    if (!data) return [];
    if (sort === "size") return data.projects;
    // Least recently active first; projects with no source files last.
    return [...data.projects].sort((a, b) => (a.lastActivity || Infinity) - (b.lastActivity || Infinity));
  }, [data, sort]);

  if (error) return <div className="placeholder">{error}</div>;
  if (!data) return <div className="placeholder">Finding projects…</div>;
  if (data.count === 0) {
    return <div className="placeholder">No projects with build or dependency folders were found here.</div>;
  }

  const inTray = new Set(trayItems?.map((i) => i.node.id));
  const inactiveTotal = data.projects
    .filter((p) => isInactive(p, nowSecs))
    .reduce((s, p) => s + p.totalAllocated, 0);

  return (
    <div className="suggestions projects">
      <header className="sugg-header">
        <div>
          <strong>{formatCount(data.count)} projects</strong> hold{" "}
          <strong>{formatBytes(data.totalAllocated)}</strong> of build and dependency folders.
          {inactiveTotal > 0 && (
            <span className="muted">
              {" "}
              {formatBytes(inactiveTotal)} of it is in projects untouched for {INACTIVE_DAYS}+ days.
            </span>
          )}
        </div>
        <div className="segmented" role="group" aria-label="Sort projects">
          <button className={sort === "size" ? "on" : ""} onClick={() => setSort("size")}>
            Largest
          </button>
          <button className={sort === "inactive" ? "on" : ""} onClick={() => setSort("inactive")}>
            Least recently active
          </button>
        </div>
      </header>
      <p className="muted small sugg-intro">
        Last activity is the newest change to a source file, ignoring build folders, dependencies and .git. Everything
        listed comes back by reinstalling or rebuilding; virtual environments are marked Review and added one at a time.
      </p>

      <ul className="project-list">
        {sorted.slice(0, shown).map((p) => {
          const safeIds = p.artifacts
            .filter((a) => a.safety === "safe" && a.addable && !inTray.has(a.node.id))
            .map((a) => a.node.id);
          const inactive = isInactive(p, nowSecs);
          return (
            <li key={p.root.id} className="project">
              <div className="project-top">
                <div className="project-name">
                  <strong>{p.root.name}</strong>
                  <span className="mono muted path" title={p.path}>
                    {p.path}
                  </span>
                </div>
                <span className={inactive ? "activity inactive" : "activity"}>
                  {p.lastActivity > 0 ? `last changed ${formatAge(p.lastActivity)}` : "no source files"}
                  {inactive && " · inactive"}
                </span>
                <span className="sugg-size">{formatBytes(p.totalAllocated)}</span>
                <button disabled={safeIds.length === 0 || cleaning} onClick={() => void addToTray(safeIds)}>
                  {safeIds.length === 0 ? "In tray" : "Add Safe artifacts"}
                </button>
              </div>
              <div className="artifacts">
                {p.artifacts.map((a) => (
                  <span
                    key={a.node.id}
                    className={`artifact ${a.safety}`}
                    title={`${a.ruleName}: ${a.path}`}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      openContextMenu({ x: e.clientX, y: e.clientY, id: a.node.id, isDir: true });
                    }}
                  >
                    <span className="mono">{a.node.name}</span> {formatBytes(a.node.sizeAllocated)}
                    {a.safety !== "safe" && <span className={`badge ${a.safety}`}>{a.safety}</span>}
                    {inTray.has(a.node.id) ? (
                      <span className="tray-badge">in tray</span>
                    ) : (
                      a.addable && (
                        <button className="small-btn" disabled={cleaning} onClick={() => void addToTray([a.node.id])}>
                          Add
                        </button>
                      )
                    )}
                  </span>
                ))}
              </div>
            </li>
          );
        })}
      </ul>
      {sorted.length > shown && (
        <button className="load-more" onClick={() => setShown(shown + PAGE)}>
          Show more ({formatCount(sorted.length - shown)} left)
        </button>
      )}
    </div>
  );
}
