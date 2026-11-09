import { useEffect, useState } from "react";
import { pickFolder } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatPercent } from "../lib/format";

export function HomeScreen() {
  const disks = useHoard((s) => s.disks);
  const home = useHoard((s) => s.home);
  const scanError = useHoard((s) => s.scanError);
  const loadTargets = useHoard((s) => s.loadTargets);
  const startScan = useHoard((s) => s.startScan);
  const [crossFs, setCrossFs] = useState(false);

  useEffect(() => {
    void loadTargets();
  }, [loadTargets]);

  const scan = (path: string) => void startScan(path, { crossFilesystems: crossFs });

  const choose = async () => {
    const path = await pickFolder();
    if (path) scan(path);
  };

  return (
    <div className="home">
      <header className="home-header">
        <h1>Hoard</h1>
        <p className="muted">Pick a drive or folder to see what is using the space.</p>
      </header>

      {scanError && (
        <div className="banner error" role="alert">
          Could not scan: {scanError}
        </div>
      )}

      <section className="targets">
        {home && (
          <button className="target-card" onClick={() => scan(home)}>
            <div className="target-title">Home folder</div>
            <div className="target-sub mono">{home}</div>
          </button>
        )}
        {disks.map((d) => {
          const pct = d.total > 0 ? (d.used / d.total) * 100 : 0;
          return (
            <button key={d.mountPoint} className="target-card" onClick={() => scan(d.mountPoint)}>
              <div className="target-title">
                {d.mountPoint}
                {d.name && <span className="muted"> {d.name}</span>}
              </div>
              <div className="usage-bar" aria-hidden="true">
                <div className="usage-fill" style={{ width: `${pct}%` }} />
              </div>
              <div className="target-sub">
                {formatBytes(d.used)} used of {formatBytes(d.total)} ({formatPercent(d.used, d.total)}),{" "}
                {formatBytes(d.available)} free
                <span className="muted">
                  {" "}
                  {d.fileSystem}
                  {d.removable ? ", removable" : ""}
                </span>
              </div>
            </button>
          );
        })}
        <button className="target-card choose" onClick={() => void choose()}>
          <div className="target-title">Choose a folder…</div>
          <div className="target-sub muted">Scan any folder on any drive</div>
        </button>
      </section>

      <label className="option">
        <input type="checkbox" checked={crossFs} onChange={(e) => setCrossFs(e.target.checked)} />
        Include other drives mounted inside the folder
      </label>
    </div>
  );
}
