// The Cleanup Tray. All destructive controls in the app live in this panel.

import { useState } from "react";
import { useHoard } from "../store";
import { formatBytes, formatCount } from "../lib/format";
import { CATEGORY_COLORS } from "../lib/colors";
import type { CleanupMode, CleanupReport, Tray } from "../types";

const IS_MAC = navigator.userAgent.includes("Mac");
const BIN = IS_MAC ? "Trash" : "Recycle Bin";

export function TrayPanel() {
  const tray = useHoard((s) => s.tray);
  const cleanup = useHoard((s) => s.cleanup);
  const removeFromTray = useHoard((s) => s.removeFromTray);
  const clearTray = useHoard((s) => s.clearTray);
  const cancelCleanup = useHoard((s) => s.cancelCleanup);
  const dismissCleanup = useHoard((s) => s.dismissCleanup);
  const [allowPermanent, setAllowPermanent] = useState(false);
  const [confirming, setConfirming] = useState<CleanupMode | null>(null);

  if (cleanup?.status === "running") {
    const p = cleanup.progress;
    const pct = p && p.total > 0 ? (p.done / p.total) * 100 : 0;
    return (
      <div className="tray">
        <h3>{cleanup.mode === "trash" ? `Moving to the ${BIN}…` : "Deleting permanently…"}</h3>
        <div className="usage-bar" aria-hidden="true">
          <div className="usage-fill" style={{ width: `${pct}%` }} />
        </div>
        <p className="muted">
          {p ? `${formatCount(p.done)} of ${formatCount(p.total)} items` : "Starting…"}
        </p>
        {p && <p className="mono current-path">{p.currentPath}</p>}
        <button onClick={() => void cancelCleanup()}>Stop after the current item</button>
      </div>
    );
  }

  if (cleanup?.status === "done") {
    return <ReportView report={cleanup.report} onDismiss={dismissCleanup} />;
  }

  return (
    <div className="tray">
      {cleanup?.status === "error" && (
        <div className="banner error" role="alert">
          Cleanup did not run: {cleanup.error}
          <button className="link" onClick={dismissCleanup}>
            Dismiss
          </button>
        </div>
      )}

      {!tray || tray.count === 0 ? (
        <div className="placeholder">
          The tray is empty. Right-click an item or select it and press Space to add it. Nothing is removed until you
          confirm here.
        </div>
      ) : (
        <>
          {tray.notes.length > 0 && (
            <ul className="tray-notes">
              {tray.notes.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          )}
          <ul className="tray-items">
            {tray.items.map((it) => (
              <li key={it.node.id}>
                <i className="swatch" style={{ background: CATEGORY_COLORS[it.node.category] }} aria-hidden="true" />
                <div className="tray-item-text">
                  <div className="name">
                    {it.node.name}
                    {it.node.kind === "dir" ? "/" : ""}
                  </div>
                  <div className="mono muted path" title={it.path}>
                    {it.path}
                  </div>
                </div>
                <span className="size">{formatBytes(it.node.sizeAllocated)}</span>
                <button className="ghost" aria-label={`Remove ${it.node.name} from the tray`} onClick={() => void removeFromTray([it.node.id])}>
                  ✕
                </button>
              </li>
            ))}
          </ul>

          <div className="tray-footer">
            <div className="tray-total">
              <strong>{formatBytes(tray.totalAllocated)}</strong> on disk (estimated) · {formatCount(tray.count)} item
              {tray.count === 1 ? "" : "s"} · {formatCount(tray.fileCount)} files
              <button className="link" onClick={() => void clearTray()}>
                Clear tray
              </button>
            </div>
            <button className="primary" onClick={() => setConfirming("trash")}>
              Move to {BIN}
            </button>
            <label className="option small">
              <input type="checkbox" checked={allowPermanent} onChange={(e) => setAllowPermanent(e.target.checked)} />
              Allow permanent delete (skips the {BIN})
            </label>
            {allowPermanent && (
              <button className="danger" onClick={() => setConfirming("permanent")}>
                Delete permanently…
              </button>
            )}
          </div>
        </>
      )}

      {confirming && tray && tray.count > 0 && (
        <ConfirmDialog tray={tray} mode={confirming} onClose={() => setConfirming(null)} />
      )}
    </div>
  );
}

function ConfirmDialog({ tray, mode, onClose }: { tray: Tray; mode: CleanupMode; onClose: () => void }) {
  const runCleanup = useHoard((s) => s.runCleanup);
  const [typed, setTyped] = useState("");
  const phrase = mode === "permanent" ? tray.permanentConfirmation : null;
  const ok = phrase === null || typed.trim() === phrase;

  const go = () => {
    onClose();
    void runCleanup(mode, phrase);
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
      >
        <h3 id="confirm-title">{mode === "trash" ? `Move to the ${BIN}?` : "Delete permanently?"}</h3>
        <p>
          {formatCount(tray.count)} item{tray.count === 1 ? "" : "s"}, {formatCount(tray.fileCount)} files,{" "}
          <strong>{formatBytes(tray.totalAllocated)}</strong> on disk (estimated).
        </p>
        {mode === "trash" ? (
          <p className="muted">
            You can restore them from the {BIN}. Disk space is freed when the {BIN} is emptied. Items that cannot go to
            the {BIN} are skipped and reported, never deleted another way.
          </p>
        ) : (
          <p className="warn-text">
            This cannot be undone. The items will not go to the {BIN}. Links are removed as links; their targets are
            not touched.
          </p>
        )}
        <p className="muted small">
          Each item is checked again just before removal. Anything protected, changed since the scan, or no longer
          inside the scanned folder is skipped.
        </p>
        {phrase !== null && (
          <label className="confirm-input">
            Type <strong className="mono">{phrase}</strong> to confirm
            <input autoFocus value={typed} onChange={(e) => setTyped(e.target.value)} spellCheck={false} />
          </label>
        )}
        <div className="modal-actions">
          <button onClick={onClose} autoFocus={phrase === null}>
            Cancel
          </button>
          <button className={mode === "trash" ? "primary" : "danger"} disabled={!ok} onClick={go}>
            {mode === "trash" ? `Move to ${BIN}` : "Delete permanently"}
          </button>
        </div>
      </div>
    </div>
  );
}

function ReportView({ report, onDismiss }: { report: CleanupReport; onDismiss: () => void }) {
  const notRemoved = report.items.filter((i) => i.status !== "removed");
  const measured = report.measuredFreed;
  return (
    <div className="tray">
      <h3>
        {report.cancelled ? "Cleanup stopped" : "Cleanup finished"}: {formatCount(report.removed)} removed
        {report.skipped > 0 && `, ${formatCount(report.skipped)} skipped`}
        {report.failed > 0 && `, ${formatCount(report.failed)} failed`}
        {report.partial > 0 && `, ${formatCount(report.partial)} partly removed`}
      </h3>
      {report.mode === "trash" ? (
        <p>
          Moved <strong>{formatBytes(report.estimatedFreed)}</strong> (estimated) to the {BIN}. The space is freed when
          you empty the {BIN}
          {measured !== null && <>; measured change so far: {formatSigned(measured)}</>}.
        </p>
      ) : (
        <p>
          Estimated <strong>{formatBytes(report.estimatedFreed)}</strong>
          {measured !== null ? (
            <>
              , measured <strong>{formatSigned(measured)}</strong> of free space
            </>
          ) : (
            ", measured free space unavailable"
          )}
          . Measured can be lower than estimated (hard links elsewhere, snapshots, other programs writing).
        </p>
      )}
      {notRemoved.length > 0 && (
        <>
          <h4>Not removed</h4>
          <ul className="report-items">
            {notRemoved.map((i) => (
              <li key={i.path}>
                <span className={`status ${i.status}`}>{i.status}</span>
                <div>
                  <div className="mono">{i.path}</div>
                  {i.reason && <div className="muted">{i.reason}</div>}
                </div>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="muted small">Every action is recorded in the action log in Hoard's app data folder.</p>
      <button onClick={onDismiss}>Done</button>
    </div>
  );
}

function formatSigned(n: number): string {
  return n < 0 ? `-${formatBytes(-n)}` : formatBytes(n);
}
