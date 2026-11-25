// Docker view (M5): what Docker's images, containers, volumes and build cache
// take, the command that frees each, and how to shrink Docker Desktop's disk
// image afterwards. Read-only: Hoard runs none of these commands.

import { useEffect, useState } from "react";
import { api, errorMessage } from "../api";
import { useHoard } from "../store";
import { formatBytes, formatCount } from "../lib/format";
import type { Docker, DockerDisk } from "../types";

const SAFETY_LABEL = { safe: "Safe", review: "Review", caution: "Caution" } as const;

export function DockerView() {
  const [docker, setDocker] = useState<Docker | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Each refresh is a new request; only the latest may update the view.
  const [request, setRequest] = useState(0);
  const [answered, setAnswered] = useState(-1);

  useEffect(() => {
    let stale = false;
    api
      .getDocker()
      .then((d) => {
        if (stale) return;
        setDocker(d);
        setError(null);
        setAnswered(request);
      })
      .catch((e: unknown) => {
        if (stale) return;
        setError(errorMessage(e));
        setAnswered(request);
      });
    return () => {
      stale = true;
    };
  }, [request]);

  const loading = answered !== request;
  const refresh = (
    <button className="small-btn" disabled={loading} onClick={() => setRequest((r) => r + 1)}>
      {loading ? "Refreshing…" : "Refresh"}
    </button>
  );

  if (!docker) {
    return (
      <div className="placeholder">
        {error ?? "Asking Docker…"} {error && refresh}
      </div>
    );
  }

  const reclaimable = docker.categories.reduce((n, c) => n + c.reclaimable, 0);
  const used = docker.categories.reduce((n, c) => n + c.size, 0);

  return (
    <div className="suggestions">
      <div className="sugg-header">
        <p className="muted small sugg-intro docker-intro">
          {docker.status === "running" &&
            `Docker uses ${formatBytes(used)}; about ${formatBytes(reclaimable)} is not in use. `}
          {docker.status === "notInstalled" && "Docker is not installed (no docker command was found). "}
          {docker.status === "unavailable" &&
            "Docker is not answering. Start Docker Desktop, then refresh, to see what is inside it. "}
          {docker.status === "timedOut" &&
            "Docker took too long to answer. It may be starting up or the computer is busy; refresh in a moment. "}
          Hoard never runs these commands; copy the ones you want into a terminal.
          {docker.status === "running" &&
            " Sizes use 1 GB = 1024 MB; docker system df counts 1000, so its numbers look about 7% larger."}
        </p>
        {refresh}
      </div>
      {error && <p className="trash-note docker-note">{error}</p>}
      {docker.status === "unavailable" && docker.reason && <p className="trash-note mono docker-note">{docker.reason}</p>}

      {docker.categories.map((c) => (
        <section key={c.name} className={`sugg-card ${c.safety}`}>
          <div className="sugg-top">
            <h3>{c.name}</h3>
            <span className={`badge ${c.safety}`}>{SAFETY_LABEL[c.safety]}</span>
            <span className="sugg-size">{formatBytes(c.size)}</span>
          </div>
          <p className="muted small">
            {formatCount(c.total)} total, {formatCount(c.active)} in use · {formatBytes(c.reclaimable)} not in use
          </p>
          <p className="small sugg-explain">{c.explain}</p>
          <Command text={c.command} />
        </section>
      ))}

      {docker.status === "running" && (
        <section className="sugg-card review">
          <div className="sugg-top">
            <h3>Everything but volumes at once</h3>
            <span className="badge review">Review</span>
          </div>
          <p className="small sugg-explain">
            Removes stopped containers, unused networks, images no container uses, and all build cache. Volumes are
            kept.
          </p>
          <Command text={docker.pruneAll} />
        </section>
      )}

      {docker.diskImages.map((d) => (
        <DiskImageCard key={d.path} disk={d} />
      ))}
    </div>
  );
}

function DiskImageCard({ disk }: { disk: DockerDisk }) {
  return (
    <section className="sugg-card">
      <div className="sugg-top">
        <h3>Docker disk image</h3>
        <span className="sugg-size">{formatBytes(disk.allocated)}</span>
      </div>
      <p className="mono small path-line" title={disk.path}>
        {disk.path}
      </p>
      {disk.compact === null ? (
        <p className="small sugg-explain">Space freed inside Docker goes back to the drive by itself.</p>
      ) : (
        <>
          <p className="small sugg-explain">
            All Docker data lives in this file. It does not shrink by itself: space freed inside Docker stays reserved
            on the drive until the file is compacted. Never delete it; that destroys everything inside.
          </p>
          <ol className="docker-steps">
            {disk.compact.map((s) => (
              <li key={s.text}>
                <span className="small">{s.text}</span>
                {s.command && <Command text={s.command} />}
              </li>
            ))}
          </ol>
        </>
      )}
    </section>
  );
}

function Command({ text }: { text: string }) {
  const showToast = useHoard((s) => s.showToast);
  const copy = () => {
    navigator.clipboard.writeText(text).then(
      () => showToast("Command copied"),
      (e: unknown) => showToast(errorMessage(e)),
    );
  };
  return (
    <div className="docker-cmd">
      <pre className="mono">{text}</pre>
      <button className="small-btn" onClick={copy} aria-label={`Copy: ${text}`}>
        Copy
      </button>
    </div>
  );
}
