import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Done, Summary } from "./types";

// The store talks to Tauri through ./api; replace it with controllable fakes.
const startScan = vi.fn<(path: string) => Promise<number>>();
vi.mock("./api", () => ({
  api: {
    startScan: (path: string) => startScan(path),
    getNode: () =>
      Promise.resolve({
        node: { id: 0, name: "root", kind: "dir", parent: null, sizeAllocated: 0, sizeApparent: 0 },
        path: "C:\\empty",
        ancestors: [],
      }),
    getChildren: () => Promise.resolve({ total: 0, offset: 0, items: [] }),
    listDisks: () => Promise.resolve([]),
    homeDir: () => Promise.resolve("C:\\Users\\me"),
  },
  errorMessage: (e: unknown) => String(e),
}));

const { useHoard } = await import("./store");

const summary: Summary = {
  rootId: 0,
  rootPath: "C:\\empty",
  sizeAllocated: 0,
  sizeApparent: 0,
  files: 0,
  dirs: 1,
  errorCount: 0,
  complete: true,
  sizeMode: "allocated",
  elapsedMs: 1,
};
const done = (scanId: number): Done => ({ scanId, summary, error: null });

describe("scan lifecycle", () => {
  beforeEach(() => {
    useHoard.setState({ screen: "home", scanId: null, earlyDone: null, summary: null });
    startScan.mockReset();
  });

  it("handles a done event that arrives before start_scan returns (empty folder)", async () => {
    let resolveId: (id: number) => void = () => {};
    startScan.mockReturnValue(new Promise((r) => (resolveId = r)));

    const started = useHoard.getState().startScan("C:\\empty");
    // The backend finishes first: done is emitted while the invoke is in flight.
    useHoard.getState().onDone(done(7));
    expect(useHoard.getState().screen).toBe("scanning");

    resolveId(7);
    await started;
    expect(useHoard.getState().screen).toBe("results");
    expect(useHoard.getState().summary?.rootPath).toBe("C:\\empty");
    expect(useHoard.getState().earlyDone).toBeNull();
  });

  it("ignores an early done that belongs to a different scan", async () => {
    let resolveId: (id: number) => void = () => {};
    startScan.mockReturnValue(new Promise((r) => (resolveId = r)));

    const started = useHoard.getState().startScan("C:\\big");
    useHoard.getState().onDone(done(3)); // stale scan
    resolveId(4);
    await started;
    expect(useHoard.getState().screen).toBe("scanning");
    expect(useHoard.getState().scanId).toBe(4);

    useHoard.getState().onDone(done(4));
    expect(useHoard.getState().screen).toBe("results");
  });

  it("handles the normal order (id first, then done)", async () => {
    startScan.mockResolvedValue(9);
    await useHoard.getState().startScan("C:\\x");
    useHoard.getState().onDone(done(9));
    expect(useHoard.getState().screen).toBe("results");
  });
});
