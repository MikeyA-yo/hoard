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

// ------------------------------------------------------------------ cleanup

export type CleanupMode = "trash" | "permanent";

export interface TrayItem {
  node: NodeInfo;
  path: string;
}

export interface Tray {
  generation: number;
  version: number;
  count: number;
  items: TrayItem[];
  totalAllocated: number;
  totalApparent: number;
  fileCount: number;
  /** Text to type before a permanent delete of this tray, if required. */
  permanentConfirmation: string | null;
  notes: string[];
}

export interface CleanupProgress {
  runId: string;
  done: number;
  total: number;
  currentPath: string;
}

export interface CleanupItem {
  path: string;
  kind: NodeKind;
  sizeAllocated: number;
  sizeApparent: number;
  status: "removed" | "skipped" | "failed" | "partial";
  reason: string | null;
}

export interface CleanupReport {
  runId: string;
  mode: CleanupMode;
  removed: number;
  skipped: number;
  failed: number;
  partial: number;
  estimatedFreed: number;
  estimatedFreedApparent: number;
  measuredFreed: number | null;
  cancelled: boolean;
  items: CleanupItem[];
  totalItems: number;
}

// ------------------------------------------------------------------ suggestions

export type Safety = "safe" | "review" | "caution";
export type RuleGroupKind = "dev" | "packages" | "caches" | "downloads" | "temp" | "system";

export interface SuggestionItem {
  node: NodeInfo;
  path: string;
  newestMtime: number;
}

export interface SuggestionGroup {
  ruleId: string;
  name: string;
  safety: Safety;
  group: RuleGroupKind;
  explain: string;
  regenerate: string | null;
  addable: boolean;
  totalAllocated: number;
  totalApparent: number;
  count: number;
  items: SuggestionItem[];
}

export interface Suggestions {
  groups: SuggestionGroup[];
  safeTotal: number;
  safeCount: number;
  reviewTotal: number;
  cautionTotal: number;
}

// ------------------------------------------------------------------ lenses

export interface CategoryTotal {
  category: Category;
  sizeAllocated: number;
  sizeApparent: number;
  files: number;
}

export interface Artifact {
  node: NodeInfo;
  path: string;
  ruleId: string;
  ruleName: string;
  safety: Safety;
  addable: boolean;
}

export interface Project {
  root: NodeInfo;
  path: string;
  /** Newest source file change, unix seconds (0 if none). */
  lastActivity: number;
  totalAllocated: number;
  totalApparent: number;
  artifacts: Artifact[];
}

export interface Projects {
  projects: Project[];
  count: number;
  totalAllocated: number;
}

export interface TrashSize {
  bytes: number;
  items: number;
}
