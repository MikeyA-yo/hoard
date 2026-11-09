// Typed wrappers over the Tauri IPC commands and scan events.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  Batch,
  ChildrenPage,
  Disk,
  Done,
  ErrorsPage,
  LargestItem,
  LargestKind,
  NodeDetail,
  Progress,
  ScanOpts,
  SizeMode,
  SortKey,
  Summary,
  TreemapData,
} from "./types";

export const api = {
  listDisks: () => invoke<Disk[]>("list_disks"),
  homeDir: () => invoke<string>("home_dir"),
  startScan: (path: string, opts?: ScanOpts) => invoke<number>("start_scan", { path, opts }),
  cancelScan: (scanId: number) => invoke<boolean>("cancel_scan", { scanId }),
  getSummary: () => invoke<Summary>("get_summary"),
  getNode: (id: number) => invoke<NodeDetail>("get_node", { id }),
  getChildren: (id: number, sort: SortKey, mode: SizeMode, offset: number, limit: number) =>
    invoke<ChildrenPage>("get_children", { id, sort, mode, offset, limit }),
  getTreemap: (id: number, depth: number, minBytes: number, mode: SizeMode, maxNodes?: number) =>
    invoke<TreemapData>("get_treemap", { id, depth, minBytes: Math.floor(minBytes), mode, maxNodes }),
  getLargest: (kind: LargestKind, limit: number, mode: SizeMode) =>
    invoke<LargestItem[]>("get_largest", { kind, limit, mode }),
  getErrors: (offset: number, limit: number) => invoke<ErrorsPage>("get_errors", { offset, limit }),
  revealNode: (id: number) => invoke<null>("reveal_node", { id }),
  openNode: (id: number) => invoke<null>("open_node", { id }),
};

/** Opens the native folder picker. Resolves to null if cancelled. */
export async function pickFolder(): Promise<string | null> {
  const result = await open({ directory: true, multiple: false, title: "Choose a folder to scan" });
  return typeof result === "string" ? result : null;
}

export interface ScanHandlers {
  progress: (p: Progress) => void;
  batch: (b: Batch) => void;
  done: (d: Done) => void;
}

/** Subscribes to all scan events. Returns one function that unsubscribes all. */
export async function onScanEvents(h: ScanHandlers): Promise<UnlistenFn> {
  const offs = await Promise.all([
    listen<Progress>("scan://progress", (e) => h.progress(e.payload)),
    listen<Batch>("scan://batch", (e) => h.batch(e.payload)),
    listen<Done>("scan://done", (e) => h.done(e.payload)),
  ]);
  return () => offs.forEach((off) => off());
}

/** Turns an IPC rejection into a readable message. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return "Something went wrong";
}
