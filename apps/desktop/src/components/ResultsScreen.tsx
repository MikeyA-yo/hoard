import { useEffect } from "react";
import { useHoard, type SideTab } from "../store";
import { formatBytes, formatCount, formatDuration } from "../lib/format";
import { sizeOf } from "../types";
import { TreemapView } from "./TreemapView";
import { ChildList } from "./ChildList";
import { ErrorsPanel, LargestPanel } from "./SidePanels";

const TABS: { key: SideTab; label: string }[] = [
  { key: "contents", label: "Contents" },
  { key: "files", label: "Largest files" },
  { key: "dirs", label: "Largest folders" },
  { key: "errors", label: "Errors" },
];

function isTyping(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

export function ResultsScreen() {
  const summary = useHoard((s) => s.summary);
  const detail = useHoard((s) => s.detail);
  const mode = useHoard((s) => s.mode);
  const colorBy = useHoard((s) => s.colorBy);
  const sideTab = useHoard((s) => s.sideTab);
  const setMode = useHoard((s) => s.setMode);
  const setColorBy = useHoard((s) => s.setColorBy);
  const setSideTab = useHoard((s) => s.setSideTab);
  const navigate = useHoard((s) => s.navigate);
  const goUp = useHoard((s) => s.goUp);
  const goHome = useHoard((s) => s.goHome);
  const startScan = useHoard((s) => s.startScan);
  const scanOpts = useHoard((s) => s.scanOpts);

  // Keyboard: arrows move selection, Enter drills in, Backspace goes up.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isTyping(e.target) || useHoard.getState().contextMenu) return;
      const s = useHoard.getState();
      switch (e.key) {
        case "ArrowDown":
        case "ArrowRight":
          e.preventDefault();
          s.moveSelection(1);
          break;
        case "ArrowUp":
        case "ArrowLeft":
          e.preventDefault();
          s.moveSelection(-1);
          break;
        case "Enter": {
          const sel = s.children.find((c) => c.id === s.selectedId);
          if (sel?.kind === "dir") {
            e.preventDefault();
            void s.navigate(sel.id);
          }
          break;
        }
        case "Backspace":
          e.preventDefault();
          s.goUp();
          break;
        case "Escape":
          s.select(null);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (!summary) return null;
  const total = mode === "allocated" ? summary.sizeAllocated : summary.sizeApparent;

  return (
    <div className="results">
      <header className="topbar">
        <button className="ghost" onClick={goHome} title="Back to drive list">
          ← Drives
        </button>
        <div className="topbar-title">
          <div className="mono root-path" title={summary.rootPath}>
            {summary.rootPath}
          </div>
          <div className="muted summary-line">
            {formatBytes(total)} {mode === "allocated" ? "on disk" : "apparent"} · {formatCount(summary.files)} files ·{" "}
            {formatCount(summary.dirs)} folders · scanned in {formatDuration(summary.elapsedMs)}
            {summary.errorCount > 0 && (
              <button className="link warn" onClick={() => setSideTab("errors")}>
                {formatCount(summary.errorCount)} unreadable
              </button>
            )}
            {!summary.complete && <span className="badge">Incomplete: scan was stopped</span>}
          </div>
        </div>
        <div className="segmented" role="group" aria-label="Size mode">
          <button className={mode === "allocated" ? "on" : ""} onClick={() => setMode("allocated")} title="Space actually used on disk">
            On disk
          </button>
          <button className={mode === "apparent" ? "on" : ""} onClick={() => setMode("apparent")} title="Logical file sizes">
            Apparent
          </button>
        </div>
        <div className="segmented" role="group" aria-label="Color by">
          <button className={colorBy === "category" ? "on" : ""} onClick={() => setColorBy("category")}>
            Type
          </button>
          <button className={colorBy === "age" ? "on" : ""} onClick={() => setColorBy("age")}>
            Age
          </button>
        </div>
        <button onClick={() => void startScan(summary.rootPath, scanOpts)}>Rescan</button>
      </header>

      <nav className="crumbs" aria-label="Breadcrumbs">
        <button className="ghost" onClick={goUp} disabled={!detail || detail.node.parent === null} title="Up one level (Backspace)">
          ↑ Up
        </button>
        {detail?.ancestors.map((c, i) => {
          const last = i === detail.ancestors.length - 1;
          return (
            <span key={c.id} className="crumb">
              {i > 0 && <span className="sep">›</span>}
              {last ? (
                <span className="current">{c.name}</span>
              ) : (
                <button className="link" onClick={() => void navigate(c.id)}>
                  {c.name}
                </button>
              )}
            </span>
          );
        })}
        {detail && <span className="crumb-size">{formatBytes(sizeOf(detail.node, mode))}</span>}
      </nav>

      <main className="workspace">
        <TreemapView />
        <aside className="side">
          <div className="tabs" role="tablist">
            {TABS.map((t) => (
              <button key={t.key} role="tab" aria-selected={sideTab === t.key} className={sideTab === t.key ? "tab on" : "tab"} onClick={() => setSideTab(t.key)}>
                {t.label}
                {t.key === "errors" && summary.errorCount > 0 ? ` (${formatCount(summary.errorCount)})` : ""}
              </button>
            ))}
          </div>
          <div className="tab-body">
            {sideTab === "contents" && <ChildList />}
            {sideTab === "files" && <LargestPanel kind="files" />}
            {sideTab === "dirs" && <LargestPanel kind="dirs" />}
            {sideTab === "errors" && <ErrorsPanel />}
          </div>
        </aside>
      </main>
    </div>
  );
}
