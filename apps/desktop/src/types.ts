// DTOs returned by the Tauri commands (see src-tauri/src/dto.rs).

export type NodeKind = "file" | "dir" | "symlink" | "other";
export type Category =
  | "folder"
  | "video"
  | "image"
  | "audio"
  | "archive"
  | "disk_image"
  | "document"
  | "code"
  | "executable"
  | "other";
export type SizeMode = "allocated" | "apparent";
export type SortKey = "size" | "count" | "modified" | "name";
export type LargestKind = "files" | "dirs";
export type ColorBy = "category" | "age";

export interface Disk {
  name: string;
  mountPoint: string;
  fileSystem: string;
  total: number;
  available: number;
  used: number;
  removable: boolean;
  readOnly: boolean;
}

export interface ScanOpts {
  crossFilesystems?: boolean;
  apparent?: boolean;
  skip?: string[];
}

export interface Progress {
  scanId: number;
  files: number;
  dirs: number;
  bytes: number;
  currentPath: string;
  errors: number;
  elapsedMs: number;
}

export interface BatchEntry {
  id: number;
  name: string;
  kind: NodeKind;
  category: Category;
  sizeAllocated: number;
  sizeApparent: number;
  fileCount: number;
}

export interface Batch {
  scanId: number;
  topLevel: BatchEntry[];
}

export interface Summary {
  rootId: number;
  rootPath: string;
  sizeAllocated: number;
  sizeApparent: number;
  files: number;
  dirs: number;
  errorCount: number;
  complete: boolean;
  sizeMode: SizeMode;
  elapsedMs: number;
}

export interface Done {
  scanId: number;
  summary: Summary | null;
  error: string | null;
}

export interface NodeInfo {
  id: number;
  name: string;
  kind: NodeKind;
  category: Category;
  sizeAllocated: number;
  sizeApparent: number;
  fileCount: number;
  childCount: number;
  mtime: number;
  flags: string[];
  parent: number | null;
}

export interface Crumb {
  id: number;
  name: string;
}

export interface NodeDetail {
  node: NodeInfo;
  path: string;
  ancestors: Crumb[];
}

export interface ChildrenPage {
  total: number;
  offset: number;
  items: NodeInfo[];
}

export interface LargestItem {
  node: NodeInfo;
  path: string;
}

export interface TreemapData {
  id: number | null;
  name: string;
  kind: NodeKind | null;
  category: Category;
  size: number;
  fileCount: number;
  mtime: number;
  mergedCount: number;
  hasMore: boolean;
  children: TreemapData[];
}

export interface ScanErrorInfo {
  path: string;
  kind: "read_dir" | "stat" | "entry";
  message: string;
}

export interface ErrorsPage {
  total: number;
  items: ScanErrorInfo[];
}

export function sizeOf(n: { sizeAllocated: number; sizeApparent: number }, mode: SizeMode): number {
  return mode === "allocated" ? n.sizeAllocated : n.sizeApparent;
}
