import { describe, expect, it } from "vitest";
import type { TreemapData } from "../types";
import { drillTarget, hitTest, layoutTreemap, minBytesFor, rectPath } from "./treemap";

function node(name: string, size: number, children: TreemapData[] = [], id: number | null = 1): TreemapData {
  return {
    id,
    name,
    kind: children.length > 0 ? "dir" : "file",
    category: children.length > 0 ? "folder" : "other",
    size,
    fileCount: 1,
    mtime: 0,
    mergedCount: 0,
    hasMore: false,
    children,
  };
}

const sample = node(
  "root",
  1000,
  [node("big", 600, [node("a", 400, [], 3), node("b", 200, [], 4)], 2), node("small", 300, [], 5)],
  0,
);

describe("layoutTreemap", () => {
  it("returns rects breadth-first with parents first", () => {
    const rects = layoutTreemap(sample, 400, 300);
    expect(rects.map((r) => r.data.name)).toEqual(["root", "big", "small", "a", "b"]);
    expect(rects[0]?.parent).toBe(-1);
    rects.slice(1).forEach((r, i) => expect(r.parent).toBeLessThan(i + 1));
  });

  it("keeps children inside their parent", () => {
    const rects = layoutTreemap(sample, 400, 300);
    for (const r of rects.slice(1)) {
      const p = rects[r.parent];
      expect(p).toBeDefined();
      if (!p) continue;
      expect(r.x).toBeGreaterThanOrEqual(p.x);
      expect(r.y).toBeGreaterThanOrEqual(p.y);
      expect(r.x + r.w).toBeLessThanOrEqual(p.x + p.w);
      expect(r.y + r.h).toBeLessThanOrEqual(p.y + p.h);
    }
  });

  it("gives areas roughly proportional to size", () => {
    const rects = layoutTreemap(sample, 1000, 1000);
    const big = rects.find((r) => r.data.name === "big");
    const small = rects.find((r) => r.data.name === "small");
    expect(big && small).toBeTruthy();
    if (!big || !small) return;
    const ratio = (big.w * big.h) / (small.w * small.h);
    expect(ratio).toBeGreaterThan(1.6);
    expect(ratio).toBeLessThan(2.4);
  });

  it("leaves room for content that was not sent (remainder)", () => {
    // Directory of 1000 bytes with a single 250-byte child: the child should
    // cover roughly a quarter of the box.
    const data = node("root", 1000, [node("only", 250, [], 9)], 0);
    const rects = layoutTreemap(data, 400, 400);
    const only = rects.find((r) => r.data.name === "only");
    expect(only).toBeDefined();
    if (!only) return;
    const share = (only.w * only.h) / (400 * 400);
    expect(share).toBeGreaterThan(0.2);
    expect(share).toBeLessThan(0.3);
  });

  it("handles empty boxes and zero sizes", () => {
    expect(layoutTreemap(sample, 0, 100)).toEqual([]);
    const zero = node("root", 0, [], 0);
    expect(layoutTreemap(zero, 100, 100)).toHaveLength(1);
  });
});

describe("hit testing", () => {
  const rects = layoutTreemap(sample, 400, 300);

  it("finds the deepest rect", () => {
    const a = rects.findIndex((r) => r.data.name === "a");
    const r = rects[a];
    expect(r).toBeDefined();
    if (!r) return;
    const hit = hitTest(rects, r.x + r.w / 2, r.y + r.h / 2);
    expect(hit).toBe(a);
    expect(rectPath(rects, hit)).toEqual(["big", "a"]);
  });

  it("drills into the nearest directory", () => {
    const a = rects.findIndex((r) => r.data.name === "a");
    const big = rects.findIndex((r) => r.data.name === "big");
    expect(drillTarget(rects, a)).toBe(2);
    expect(drillTarget(rects, big)).toBe(2);
    const small = rects.findIndex((r) => r.data.name === "small");
    expect(drillTarget(rects, small)).toBeNull();
  });

  it("misses outside", () => {
    expect(hitTest(rects, -5, -5)).toBe(-1);
    expect(hitTest(rects, 400, 10)).toBe(-1);
  });
});

describe("minBytesFor", () => {
  it("scales with size and area", () => {
    expect(minBytesFor(1_000_000, 100, 100, 10)).toBe(1000);
    expect(minBytesFor(0, 100, 100)).toBe(0);
    expect(minBytesFor(100, 0, 100)).toBe(0);
  });
});
