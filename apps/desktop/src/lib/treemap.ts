// Treemap layout (d3-hierarchy squarify) and hit testing. Pure functions so
// they can be unit tested without a canvas.

import { hierarchy, treemap, treemapSquarify, type HierarchyRectangularNode } from "d3-hierarchy";
import type { TreemapData } from "../types";

/** Height of the name strip drawn on top of directories with room for it. */
export const HEADER_HEIGHT = 16;

export interface TreemapRect {
  x: number;
  y: number;
  w: number;
  h: number;
  depth: number;
  data: TreemapData;
  /** Index of the parent rect, -1 for the root. */
  parent: number;
  /** True if this rect has a header strip (directory with laid-out children). */
  header: boolean;
  /** Whether children were laid out inside this rect. */
  hasChildren: boolean;
}

type Node = HierarchyRectangularNode<TreemapData>;

function wantsHeader(n: Node): boolean {
  return n.depth > 0 && !!n.children && n.y1 - n.y0 > HEADER_HEIGHT * 2.5 && n.x1 - n.x0 > 48;
}

/**
 * Lays out `data` in a `width` x `height` box. A directory's size can exceed
 * the sum of its included children (merged or unexpanded content); that
 * remainder is left as empty space inside the directory so areas stay
 * proportional to real sizes.
 *
 * Rects are returned breadth-first, so every parent precedes its children
 * and, along any root-to-leaf chain, deeper rects come later.
 */
export function layoutTreemap(data: TreemapData, width: number, height: number): TreemapRect[] {
  if (width <= 0 || height <= 0) return [];
  const root = hierarchy<TreemapData>(data, (d) => (d.children.length > 0 ? d.children : undefined))
    .sum((d) => Math.max(0, d.size - d.children.reduce((s, c) => s + c.size, 0)))
    .sort((a, b) => (b.value ?? 0) - (a.value ?? 0));

  const layout = treemap<TreemapData>()
    .tile(treemapSquarify)
    .size([width, height])
    .round(true)
    .paddingInner(1)
    .paddingOuter((n) => (n.depth === 0 ? 0 : 2))
    .paddingTop((n) => (wantsHeader(n) ? HEADER_HEIGHT : n.depth === 0 ? 0 : 2));

  const laid = layout(root);
  const nodes = laid.descendants();
  const index = new Map<Node, number>();
  nodes.forEach((n, i) => index.set(n, i));

  return nodes.map((n) => ({
    x: n.x0,
    y: n.y0,
    w: Math.max(0, n.x1 - n.x0),
    h: Math.max(0, n.y1 - n.y0),
    depth: n.depth,
    data: n.data,
    parent: n.parent ? (index.get(n.parent) ?? -1) : -1,
    header: wantsHeader(n),
    hasChildren: !!n.children,
  }));
}

/** Index of the deepest rect containing (x, y), or -1. */
export function hitTest(rects: TreemapRect[], x: number, y: number): number {
  for (let i = rects.length - 1; i >= 0; i--) {
    const r = rects[i];
    if (r && r.w > 0 && r.h > 0 && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) return i;
  }
  return -1;
}

/** Names from the root (exclusive) down to rect `i` (inclusive). */
export function rectPath(rects: TreemapRect[], i: number): string[] {
  const names: string[] = [];
  let cur = i;
  while (cur > 0) {
    const r = rects[cur];
    if (!r) break;
    names.push(r.data.name);
    cur = r.parent;
  }
  return names.reverse();
}

/**
 * Nearest enclosing rect (including `i` itself) that refers to a real
 * directory, excluding the root. Used to decide what a double-click drills into.
 */
export function drillTarget(rects: TreemapRect[], i: number): number | null {
  let cur = i;
  while (cur > 0) {
    const r = rects[cur];
    if (!r) return null;
    if (r.data.id !== null && r.data.kind === "dir") return r.data.id;
    cur = r.parent;
  }
  return null;
}

/**
 * Smallest size worth sending for a treemap of `totalBytes` in a box of
 * `width` x `height` px: anything that would cover less than `minArea` px is
 * merged server-side.
 */
export function minBytesFor(totalBytes: number, width: number, height: number, minArea = 24): number {
  const area = width * height;
  if (area <= 0 || totalBytes <= 0) return 0;
  return Math.floor((totalBytes * minArea) / area);
}
