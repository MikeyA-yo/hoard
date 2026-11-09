import { describe, expect, it } from "vitest";
import type { DupFile } from "../types";
import { suggestKeeper } from "./dupes";

function file(id: number, path: string, mtime: number): DupFile {
  return {
    path,
    node: {
      id,
      name: path.split(/[\\/]/).pop() ?? "",
      kind: "file",
      category: "video",
      sizeAllocated: 1,
      sizeApparent: 1,
      fileCount: 1,
      childCount: 0,
      mtime,
      flags: [],
      parent: null,
    },
  };
}

describe("suggestKeeper", () => {
  it("prefers a copy outside Downloads and Temp", () => {
    const files = [
      file(1, "C:\\Users\\me\\Downloads\\movie.mkv", 100),
      file(2, "C:\\Users\\me\\Videos\\movie.mkv", 500),
      file(3, "C:\\Users\\me\\AppData\\Local\\Temp\\movie.mkv", 50),
    ];
    expect(suggestKeeper(files)).toBe(2);
  });

  it("then the oldest, then the shortest path", () => {
    expect(suggestKeeper([file(1, "C:\\a\\x.bin", 200), file(2, "C:\\bb\\x.bin", 100)])).toBe(2);
    expect(suggestKeeper([file(1, "C:\\long\\path\\x.bin", 100), file(2, "C:\\s\\x.bin", 100)])).toBe(2);
  });

  it("handles empty groups", () => {
    expect(suggestKeeper([])).toBeNull();
  });
});
