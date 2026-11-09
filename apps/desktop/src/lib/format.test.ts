import { describe, expect, it } from "vitest";
import {
  formatAge,
  formatBytes,
  formatCount,
  formatDuration,
  formatPercent,
  joinPath,
  separatorFor,
} from "./format";

describe("formatBytes", () => {
  it("matches the Rust formatter", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1024 * 1024 - 1)).toBe("1.0 MB");
    expect(formatBytes(1.5 * 1024 ** 3)).toBe("1.5 GB");
  });
  it("supports IEC labels", () => {
    expect(formatBytes(1024 ** 2, true)).toBe("1.0 MiB");
  });
  it("rejects nonsense", () => {
    expect(formatBytes(-1)).toBe("?");
    expect(formatBytes(Number.NaN)).toBe("?");
  });
});

describe("other formatters", () => {
  it("counts", () => {
    expect(formatCount(1234567)).toBe("1,234,567");
  });
  it("percent", () => {
    expect(formatPercent(1, 4)).toBe("25.0%");
    expect(formatPercent(1, 10_000)).toBe("<0.1%");
    expect(formatPercent(0, 0)).toBe("0%");
  });
  it("age", () => {
    const now = 1_000_000_000_000;
    expect(formatAge(now / 1000 - 30, now)).toBe("just now");
    expect(formatAge(now / 1000 - 3 * 86_400, now)).toBe("3 days ago");
    expect(formatAge(now / 1000 - 400 * 86_400, now)).toBe("1 year ago");
    expect(formatAge(0, now)).toBe("");
  });
  it("duration", () => {
    expect(formatDuration(4200)).toBe("4.2 s");
    expect(formatDuration(125_000)).toBe("2 min 05 s");
  });
  it("paths", () => {
    expect(separatorFor("C:\\Users\\x")).toBe("\\");
    expect(separatorFor("/home/x")).toBe("/");
    expect(joinPath("C:\\", ["Users", "x"])).toBe("C:\\Users\\x");
    expect(joinPath("/home/me", ["a", "b"])).toBe("/home/me/a/b");
  });
});
