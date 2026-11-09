// The guide: reclaimable space found by the rules, largest first.
// Adding only stages items in the Cleanup Tray; nothing is removed here.

import { useState } from "react";
import { useHoard } from "../store";
import { formatAge, formatBytes, formatCount } from "../lib/format";
import type { RuleGroupKind, Safety, SuggestionGroup } from "../types";

type Filter = "all" | "dev" | "caches" | "downloads";
const FILTERS: { key: Filter; label: string; groups: RuleGroupKind[] }[] = [
  { key: "all", label: "All", groups: ["dev", "packages", "caches", "downloads", "temp", "system"] },
  { key: "dev", label: "Dev projects", groups: ["dev"] },
  { key: "caches", label: "Caches & system", groups: ["packages", "caches", "temp", "system"] },
  { key: "downloads", label: "Downloads", groups: ["downloads"] },
];

const SAFETY_LABEL: Record<Safety, string> = { safe: "Safe", review: "Review", caution: "Caution" };
const SAFETY_HELP: Record<Safety, string> = {
  safe: "Comes back automatically or on demand. Nothing personal.",
  review: "Usually fine to remove, but look first. Added one at a time.",
  caution: "Can lose data. Read the note before doing anything.",
};
const SHOWN_AT_FIRST = 8;

export function SuggestionsView() {
  const suggestions = useHoard((s) => s.suggestions);
  const loading = useHoard((s) => s.suggestionsLoading);
  const loadSuggestions = useHoard((s) => s.loadSuggestions);
  const addSuggestions = useHoard((s) => s.addSuggestions);
  const cleaning = useHoard((s) => s.cleanup?.status === "running");
  const [filter, setFilter] = useState<Filter>("all");

  if (!suggestions) {
    return <div className="placeholder">{loading ? "Looking for reclaimable space…" : "No suggestions loaded."}</div>;
  }

  return (
    <div className="suggestions">
      <header className="sugg-header">
        <div className="sugg-totals">
          <span className="chip safe">Safe {formatBytes(suggestions.safeTotal)}</span>
          <span className="chip review">Review {formatBytes(suggestions.reviewTotal)}</span>
          {suggestions.cautionTotal > 0 && (
            <span className="chip caution">Caution {formatBytes(suggestions.cautionTotal)}</span>
          )}
          <button className="link" onClick={() => void loadSuggestions()} disabled={loading}>
            {loading ? "Refreshing…" : "Refresh"}
          </button>
        </div>
        <button
          className="primary"
          disabled={suggestions.safeCount === 0 || cleaning}
          onClick={() => void addSuggestions(null)}
        >
          Add all Safe items to tray ({formatBytes(suggestions.safeTotal)})
        </button>
      </header>
      <p className="muted small sugg-intro">
        Adding only puts items in the Cleanup Tray on the right. Nothing is removed until you confirm there, and every
        item is checked again just before removal. Sizes are on disk, as of the scan.
      </p>

      <div className="segmented sugg-filter" role="group" aria-label="Show">
        {FILTERS.map((f) => (
          <button key={f.key} className={filter === f.key ? "on" : ""} onClick={() => setFilter(f.key)}>
            {f.label}
          </button>
        ))}
      </div>

      {suggestions.groups.length === 0 ? (
        <div className="placeholder">Nothing here matched the built-in rules. Try the Map view or Largest files.</div>
      ) : (
        suggestions.groups
          .filter((g) => FILTERS.find((f) => f.key === filter)?.groups.includes(g.group))
          .map((g) => <GroupCard key={g.ruleId} group={g} disabled={cleaning} />)
      )}
    </div>
  );
}

function GroupCard({ group: g, disabled }: { group: SuggestionGroup; disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [shown, setShown] = useState(SHOWN_AT_FIRST);
  const trayItems = useHoard((s) => s.tray?.items);
  const addToTray = useHoard((s) => s.addToTray);
  const addSuggestions = useHoard((s) => s.addSuggestions);
  const openContextMenu = useHoard((s) => s.openContextMenu);
  const inTray = new Set(trayItems?.map((i) => i.node.id));
  const bulk = g.safety === "safe" && g.addable;

  return (
    <section className={`sugg-card ${g.safety}`}>
      <div className="sugg-top">
        <span className={`badge ${g.safety}`} title={SAFETY_HELP[g.safety]}>
          {SAFETY_LABEL[g.safety]}
        </span>
        <h3>{g.name}</h3>
        <span className="sugg-size">{formatBytes(g.totalAllocated)}</span>
        <span className="muted">
          {formatCount(g.count)} item{g.count === 1 ? "" : "s"}
        </span>
      </div>
      <p className="sugg-explain">{g.explain}</p>
      {g.regenerate && (
        <p className="muted small">
          {g.addable ? "Comes back: " : "Instead: "}
          {g.regenerate}
        </p>
      )}
      <div className="sugg-actions">
        {bulk && (
          <button disabled={disabled} onClick={() => void addSuggestions(g.ruleId)}>
            Add all {formatCount(g.count)} to tray
          </button>
        )}
        {!g.addable && <span className="info-only">Information only: Hoard will not remove this.</span>}
        <button className="link" onClick={() => setOpen(!open)} aria-expanded={open}>
          {open ? "Hide items" : g.safety === "safe" ? "Show items" : "Review items"}
        </button>
      </div>

      {open && (
        <ul className="sugg-items">
          {g.items.slice(0, shown).map((it) => (
            <li
              key={it.node.id}
              onContextMenu={(e) => {
                e.preventDefault();
                openContextMenu({ x: e.clientX, y: e.clientY, id: it.node.id, isDir: it.node.kind === "dir" });
              }}
            >
              <span className="size">{formatBytes(it.node.sizeAllocated)}</span>
              <span className="mono path" title={it.path}>
                {it.path}
              </span>
              <span className="muted age">{it.newestMtime > 0 ? `changed ${formatAge(it.newestMtime)}` : ""}</span>
              {g.addable &&
                (inTray.has(it.node.id) ? (
                  <span className="tray-badge">in tray</span>
                ) : (
                  <button className="small-btn" disabled={disabled} onClick={() => void addToTray([it.node.id])}>
                    Add
                  </button>
                ))}
            </li>
          ))}
          {g.items.length > shown && (
            <li className="more">
              <button className="link" onClick={() => setShown(shown + 50)}>
                Show more ({formatCount(g.items.length - shown)} left)
              </button>
            </li>
          )}
          {g.count > g.items.length && (
            <li className="more muted small">
              {formatCount(g.count - g.items.length)} smaller items are not listed
              {bulk ? "; Add all includes them." : "."}
            </li>
          )}
        </ul>
      )}
    </section>
  );
}
