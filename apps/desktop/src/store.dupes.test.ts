import { describe, expect, it, vi } from "vitest";
import type { DupesSummary } from "./types";

const findDuplicates = vi.fn<(runId: string, minSize: number) => Promise<DupesSummary>>();
vi.mock("./api", () => ({
  api: { findDuplicates: (r: string, m: number) => findDuplicates(r, m) },
  errorMessage: (e: unknown) => String(e),
}));

const { useHoard } = await import("./store");

describe("duplicate search run", () => {
  it("uses a caller-made run id, filters progress by it and takes the result from the call", async () => {
    let resolve: (s: DupesSummary) => void = () => {};
    let seen = "";
    findDuplicates.mockImplementation((runId) => {
      seen = runId;
      useHoard.getState().onDupesProgress({ runId, stage: 2, filesDone: 3, filesTotal: 9, bytesDone: 1, bytesTotal: 2 });
      useHoard.getState().onDupesProgress({ runId: "other", stage: 1, filesDone: 0, filesTotal: 0, bytesDone: 0, bytesTotal: 0 });
      return new Promise((r) => (resolve = r));
    });
    const before = useHoard.getState().dupesVersion;
    const running = useHoard.getState().findDuplicates(1024);
    const run = useHoard.getState().dupesRun;
    expect(run?.runId).toBe(seen);
    expect(run?.status === "running" && run.progress?.filesDone).toBe(3);

    // A second search while one runs is ignored.
    void useHoard.getState().findDuplicates(1024);
    expect(findDuplicates).toHaveBeenCalledTimes(1);

    resolve({ runId: seen, groups: 2, wasted: 100, unreadable: 0, cancelled: false });
    await running;
    expect(useHoard.getState().dupesRun?.status).toBe("done");
    expect(useHoard.getState().dupesVersion).toBe(before + 1);
  });
});
