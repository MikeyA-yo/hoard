import { create } from "zustand";
import { api, errorMessage } from "./api";
import type {
  BatchEntry,
  ColorBy,
  Disk,
  Done,
  NodeDetail,
  NodeInfo,
  Progress,
  ScanOpts,
  SizeMode,
  SortKey,
  Summary,
  Batch,
} from "./types";

export type Screen = "home" | "scanning" | "results";
export type SideTab = "contents" | "files" | "dirs" | "errors";

const PAGE_SIZE = 300;

export interface ContextMenuState {
  x: number;
  y: number;
  id: number;
  isDir: boolean;
}

interface HoardState {
  screen: Screen;
  disks: Disk[];
  home: string | null;

  scanPath: string | null;
  scanOpts: ScanOpts;
  scanId: number | null;
  progress: Progress | null;
  batch: BatchEntry[];
  scanError: string | null;
  /**
   * A `scan://done` that arrived before `start_scan` returned its id. Tiny
   * scans (an empty folder takes ~1 ms) finish before the IPC reply lands, so
   * the event is held here and matched once the id is known.
   */
  earlyDone: Done | null;

  summary: Summary | null;
  currentId: number | null;
  detail: NodeDetail | null;
  children: NodeInfo[];
  childrenTotal: number;
  selectedId: number | null;
  sort: SortKey;
  mode: SizeMode;
  colorBy: ColorBy;
  sideTab: SideTab;
  contextMenu: ContextMenuState | null;
  toast: string | null;

  loadTargets: () => Promise<void>;
  startScan: (path: string, opts?: ScanOpts) => Promise<void>;
  cancelScan: () => Promise<void>;
  onProgress: (p: Progress) => void;
  onBatch: (b: Batch) => void;
  onDone: (d: Done) => void;
  goHome: () => void;

  navigate: (id: number) => Promise<void>;
  goUp: () => void;
  loadMoreChildren: () => Promise<void>;
  select: (id: number | null) => void;
  moveSelection: (delta: number) => void;
  setSort: (sort: SortKey) => void;
  setMode: (mode: SizeMode) => void;
  setColorBy: (c: ColorBy) => void;
  setSideTab: (t: SideTab) => void;
  openContextMenu: (m: ContextMenuState | null) => void;
  showToast: (msg: string) => void;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

export const useHoard = create<HoardState>()((set, get) => ({
  screen: "home",
  disks: [],
  home: null,

  scanPath: null,
  scanOpts: {},
  scanId: null,
  progress: null,
  batch: [],
  scanError: null,
  earlyDone: null,

  summary: null,
  currentId: null,
  detail: null,
  children: [],
  childrenTotal: 0,
  selectedId: null,
  sort: "size",
  mode: "allocated",
  colorBy: "category",
  sideTab: "contents",
  contextMenu: null,
  toast: null,

  loadTargets: async () => {
    try {
      const [disks, home] = await Promise.all([api.listDisks(), api.homeDir()]);
      set({ disks, home });
    } catch (e) {
      get().showToast(errorMessage(e));
    }
  },

  startScan: async (path, opts = {}) => {
    set({
      screen: "scanning",
      scanPath: path,
      scanOpts: opts,
      scanId: null,
      progress: null,
      batch: [],
      scanError: null,
      earlyDone: null,
      contextMenu: null,
    });
    try {
      const scanId = await api.startScan(path, opts);
      const early = get().earlyDone;
      set({ scanId, earlyDone: null });
      if (early && early.scanId === scanId) get().onDone(early);
    } catch (e) {
      set({ screen: "home", scanError: errorMessage(e) });
    }
  },

  cancelScan: async () => {
    const { scanId } = get();
    if (scanId !== null) await api.cancelScan(scanId);
  },

  onProgress: (p) => {
    if (p.scanId === get().scanId) set({ progress: p });
  },

  onBatch: (b) => {
    if (b.scanId === get().scanId) set({ batch: b.topLevel });
  },

  onDone: (d) => {
    const { scanId, screen } = get();
    if (scanId === null && screen === "scanning") {
      // Our start_scan call has not returned yet; keep this until it does.
      set({ earlyDone: d });
      return;
    }
    if (d.scanId !== scanId) return;
    if (d.error || !d.summary) {
      set({ screen: "home", scanError: d.error ?? "Scan failed", scanId: null });
      return;
    }
    set({
      screen: "results",
      summary: d.summary,
      scanId: null,
      sideTab: "contents",
      // Show sizes the way the scan was asked for; the toggle can change it.
      mode: d.summary.sizeMode,
    });
    void get().navigate(d.summary.rootId);
  },

  goHome: () => {
    set({ screen: "home", contextMenu: null });
    void get().loadTargets();
  },

  navigate: async (id) => {
    set({ currentId: id, selectedId: null, contextMenu: null });
    const { sort, mode } = get();
    try {
      const [detail, page] = await Promise.all([api.getNode(id), api.getChildren(id, sort, mode, 0, PAGE_SIZE)]);
      if (get().currentId !== id) return;
      set({ detail, children: page.items, childrenTotal: page.total, sideTab: "contents" });
    } catch (e) {
      get().showToast(errorMessage(e));
    }
  },

  goUp: () => {
    const parent = get().detail?.node.parent;
    if (parent !== null && parent !== undefined) void get().navigate(parent);
  },

  loadMoreChildren: async () => {
    const { currentId, sort, mode, children } = get();
    if (currentId === null) return;
    try {
      const page = await api.getChildren(currentId, sort, mode, children.length, PAGE_SIZE);
      if (get().currentId !== currentId) return;
      set({ children: [...children, ...page.items], childrenTotal: page.total });
    } catch (e) {
      get().showToast(errorMessage(e));
    }
  },

  select: (id) => set({ selectedId: id }),

  moveSelection: (delta) => {
    const { children, selectedId } = get();
    if (children.length === 0) return;
    const idx = children.findIndex((c) => c.id === selectedId);
    const next = idx < 0 ? (delta > 0 ? 0 : children.length - 1) : Math.max(0, Math.min(children.length - 1, idx + delta));
    const node = children[next];
    if (node) set({ selectedId: node.id });
  },

  setSort: (sort) => {
    set({ sort });
    const { currentId } = get();
    if (currentId !== null) void get().navigate(currentId);
  },

  setMode: (mode) => {
    set({ mode });
    const { currentId } = get();
    if (currentId !== null) void get().navigate(currentId);
  },

  setColorBy: (colorBy) => set({ colorBy }),
  setSideTab: (sideTab) => set({ sideTab }),
  openContextMenu: (contextMenu) => set({ contextMenu }),

  showToast: (msg) => {
    set({ toast: msg });
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => set({ toast: null }), 3500);
  },
}));

/** Reveal / open / copy actions shared by the treemap, list and panels. */
export const nodeActions = {
  reveal: async (id: number) => {
    try {
      await api.revealNode(id);
    } catch (e) {
      useHoard.getState().showToast(errorMessage(e));
    }
  },
  open: async (id: number) => {
    try {
      await api.openNode(id);
    } catch (e) {
      useHoard.getState().showToast(errorMessage(e));
    }
  },
  copyPath: async (id: number) => {
    try {
      const { path } = await api.getNode(id);
      await navigator.clipboard.writeText(path);
      useHoard.getState().showToast("Path copied");
    } catch (e) {
      useHoard.getState().showToast(errorMessage(e));
    }
  },
};
