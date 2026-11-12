import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CleanupReport, Tray } from "./types";

const executeCleanup = vi.fn<(runId: string, mode: string, confirmation: string | null) => Promise<CleanupReport>>();
const trayAdd = vi.fn<(ids: number[]) => Promise<Tray>>();

vi.mock("./api", () => ({
  api: {
    executeCleanup: (runId: string, mode: string, c: string | null) => executeCleanup(runId, mode, c),
    trayAdd: (ids: number[]) => trayAdd(ids),
    trayList: () => Promise.resolve(tray(1, 9, 0)),
    getSummary: () =>
      Promise.resolve({
        rootId: 0,
        rootPath: "C:\\x",
        sizeAllocated: 0,
        sizeApparent: 0,
        files: 0,
        dirs: 1,
        errorCount: 0,
        complete: true,
        sizeMode: "allocated",
        elapsedMs: 1,
        source: "walk",
      }),
    getNode: () => Promise.resolve({ node: { id: 0, flags: [], parent: null }, path: "C:\\x", ancestors: [] }),
    getChildren: () => Promise.resolve({ total: 0, offset: 0, items: [] }),
  },
  errorMessage: (e: unknown) => String(e),
}));

const { useHoard, isNewerTray } = await import("./store");

function tray(generation: number, version: number, count: number): Tray {
  return {
    generation,
    version,
    count,
    items: [],
    totalAllocated: 0,
    totalApparent: 0,
    fileCount: 0,
    permanentConfirmation: null,
    notes: [],
  };
}

function report(runId: string): CleanupReport {
  return {
    runId,
    mode: "trash",
    removed: 1,
    skipped: 0,
    failed: 0,
    partial: 0,
    estimatedFreed: 10,
    estimatedFreedApparent: 10,
    measuredFreed: 0,
    cancelled: false,
    items: [],
    totalItems: 1,
  };
}

describe("tray versioning", () => {
  it("only applies newer responses", () => {
    expect(isNewerTray(null, tray(1, 1, 0))).toBe(true);
    expect(isNewerTray(tray(1, 5, 2), tray(1, 6, 3))).toBe(true);
    expect(isNewerTray(tray(1, 5, 2), tray(1, 4, 1))).toBe(false);
    // A new scan's tray wins even with a lower version.
    expect(isNewerTray(tray(1, 9, 2), tray(2, 1, 0))).toBe(true);
    expect(isNewerTray(tray(2, 1, 0), tray(1, 9, 2))).toBe(false);
  });

  it("ignores a slow, older tray response that lands last", () => {
    useHoard.setState({ tray: null });
    useHoard.getState().applyTray(tray(1, 3, 2));
    useHoard.getState().applyTray(tray(1, 2, 1)); // out of order
    expect(useHoard.getState().tray?.version).toBe(3);
  });
});

describe("cleanup run", () => {
  beforeEach(() => {
    useHoard.setState({ cleanup: null, currentId: null, tray: null });
    executeCleanup.mockReset();
  });

  it("knows its run id before the backend is called and filters progress by it", async () => {
    let resolve: (r: CleanupReport) => void = () => {};
    let seenId = "";
    executeCleanup.mockImplementation((runId) => {
      seenId = runId;
      // A progress event for this run arrives while the call is in flight.
      useHoard.getState().onCleanupProgress({ runId, done: 1, total: 2, currentPath: "C:\\x\\a" });
      // One for some other run is ignored.
      useHoard.getState().onCleanupProgress({ runId: "other", done: 9, total: 9, currentPath: "nope" });
      return new Promise((r) => (resolve = r));
    });

    const running = useHoard.getState().runCleanup("trash", null);
    const run = useHoard.getState().cleanup;
    expect(run?.status).toBe("running");
    expect(run?.runId).toBe(seenId);
    expect(run?.status === "running" && run.progress?.currentPath).toBe("C:\\x\\a");

    resolve(report(seenId));
    await running;
    const done = useHoard.getState().cleanup;
    expect(done?.status).toBe("done");
    expect(done?.status === "done" && done.report.removed).toBe(1);
  });

  it("does not start a second run while one is running", async () => {
    executeCleanup.mockReturnValue(new Promise(() => {}));
    void useHoard.getState().runCleanup("trash", null);
    void useHoard.getState().runCleanup("permanent", "1 KB");
    expect(executeCleanup).toHaveBeenCalledTimes(1);
  });

  it("shows a rejected run (e.g. confirmation missing) as an error", async () => {
    executeCleanup.mockRejectedValue("type 10.0 GB to confirm permanent deletion");
    await useHoard.getState().runCleanup("permanent", null);
    const run = useHoard.getState().cleanup;
    expect(run?.status).toBe("error");
    expect(run?.status === "error" && run.error).toContain("10.0 GB");
  });
});
