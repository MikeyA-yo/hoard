import { describe, expect, it, vi } from "vitest";
import type { Suggestions } from "./types";

const getSuggestions = vi.fn<() => Promise<Suggestions>>();
vi.mock("./api", () => ({
  api: { getSuggestions: () => getSuggestions() },
  errorMessage: (e: unknown) => String(e),
}));

const { useHoard } = await import("./store");

const result = (safeTotal: number): Suggestions => ({
  groups: [],
  safeTotal,
  safeCount: 0,
  reviewTotal: 0,
  cautionTotal: 0,
});

describe("suggestions loading", () => {
  it("ignores a slow response from an older request", async () => {
    let resolveOld: (s: Suggestions) => void = () => {};
    let resolveNew: (s: Suggestions) => void = () => {};
    getSuggestions
      .mockReturnValueOnce(new Promise((r) => (resolveOld = r)))
      .mockReturnValueOnce(new Promise((r) => (resolveNew = r)));

    const first = useHoard.getState().loadSuggestions();
    const second = useHoard.getState().loadSuggestions();
    resolveNew(result(2));
    await second;
    resolveOld(result(1)); // lands last, but is stale
    await first;

    expect(useHoard.getState().suggestions?.safeTotal).toBe(2);
    expect(useHoard.getState().suggestionsLoading).toBe(false);
  });
});
