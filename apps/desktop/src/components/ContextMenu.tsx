import { useEffect, useRef } from "react";
import { nodeActions, useHoard } from "../store";

const REVEAL_LABEL = navigator.userAgent.includes("Mac") ? "Reveal in Finder" : "Show in Explorer";

export function ContextMenu() {
  const menu = useHoard((s) => s.contextMenu);
  const close = useHoard((s) => s.openContextMenu);
  const navigate = useHoard((s) => s.navigate);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menu) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close(null);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close(null);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    ref.current?.querySelector("button")?.focus();
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu, close]);

  if (!menu) return null;

  const run = (fn: () => void) => () => {
    close(null);
    fn();
  };

  // Keep the menu on screen.
  const x = Math.min(menu.x, window.innerWidth - 200);
  const y = Math.min(menu.y, window.innerHeight - 160);

  return (
    <div ref={ref} className="context-menu" style={{ left: x, top: y }} role="menu">
      {menu.isDir && (
        <button role="menuitem" onClick={run(() => void navigate(menu.id))}>
          Open folder here
        </button>
      )}
      <button role="menuitem" onClick={run(() => void nodeActions.reveal(menu.id))}>
        {REVEAL_LABEL}
      </button>
      <button role="menuitem" onClick={run(() => void nodeActions.open(menu.id))}>
        Open
      </button>
      <button role="menuitem" onClick={run(() => void nodeActions.copyPath(menu.id))}>
        Copy path
      </button>
    </div>
  );
}
