import { useEffect, useMemo, useRef, useState } from "react";
import type { ColorBy, TreemapData } from "../types";
import { formatBytes } from "../lib/format";
import { CATEGORY_COLORS, MERGED_COLOR, ageColor, shade, textColorOn } from "../lib/colors";
import { HEADER_HEIGHT, drillTarget, hitTest, layoutTreemap, rectPath, type TreemapRect } from "../lib/treemap";

export interface HoverInfo {
  name: string;
  /** Names from the treemap root (exclusive) to the hovered node. */
  pathParts: string[];
  size: number;
  fileCount: number;
  mtime: number;
  kind: TreemapData["kind"];
  mergedCount: number;
}

interface Props {
  data: TreemapData | null;
  colorBy: ColorBy;
  selectedId?: number | null;
  interactive?: boolean;
  onSelect?: (id: number | null) => void;
  onDrill?: (id: number) => void;
  onHover?: (info: HoverInfo | null) => void;
  onContextMenu?: (x: number, y: number, id: number, isDir: boolean) => void;
  onResize?: (w: number, h: number) => void;
}

const FONT = "11px system-ui, -apple-system, 'Segoe UI', sans-serif";
const BG = "#14171f";

function fillFor(r: TreemapRect, colorBy: ColorBy): string {
  const d = r.data;
  if (d.id === null) return MERGED_COLOR;
  if (r.hasChildren) return shade(CATEGORY_COLORS.folder, Math.min(0.3, r.depth * 0.06));
  if (colorBy === "age") return ageColor(d.mtime);
  if (d.kind === "dir") return shade(CATEGORY_COLORS.folder, 0.22);
  return CATEGORY_COLORS[d.category];
}

function fitText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string {
  if (maxWidth <= 8) return "";
  if (ctx.measureText(text).width <= maxWidth) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (ctx.measureText(`${text.slice(0, mid)}…`).width <= maxWidth) lo = mid;
    else hi = mid - 1;
  }
  return lo > 0 ? `${text.slice(0, lo)}…` : "";
}

export function Treemap({
  data,
  colorBy,
  selectedId = null,
  interactive = true,
  onSelect,
  onDrill,
  onHover,
  onContextMenu,
  onResize,
}: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hoverIdx, setHoverIdx] = useState(-1);

  // Track the container size.
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (!box) return;
      const w = Math.floor(box.width);
      const h = Math.floor(box.height);
      setSize((s) => (s.w === w && s.h === h ? s : { w, h }));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (size.w > 0 && size.h > 0) onResize?.(size.w, size.h);
  }, [size, onResize]);

  const rects = useMemo(() => (data ? layoutTreemap(data, size.w, size.h) : []), [data, size]);

  // Reset hover when the layout changes. setState during render keyed on the
  // rects identity avoids an extra effect pass.
  const [prevRects, setPrevRects] = useState(rects);
  if (prevRects !== rects) {
    setPrevRects(rects);
    setHoverIdx(-1);
  }

  // Draw.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.max(1, Math.floor(size.w * dpr));
    canvas.height = Math.max(1, Math.floor(size.h * dpr));
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = BG;
    ctx.fillRect(0, 0, size.w, size.h);
    ctx.font = FONT;
    ctx.textBaseline = "middle";

    rects.forEach((r, i) => {
      if (r.w < 1 || r.h < 1) return;
      if (i === 0 && r.hasChildren) return; // root background stays BG
      const fill = fillFor(r, colorBy);
      ctx.fillStyle = fill;
      ctx.fillRect(r.x, r.y, r.w, r.h);
      const fg = textColorOn(fill);

      if (r.header) {
        ctx.fillStyle = fg;
        const label = `${r.data.name}  ${formatBytes(r.data.size)}`;
        ctx.fillText(fitText(ctx, label, r.w - 8), r.x + 4, r.y + HEADER_HEIGHT / 2 + 1);
      } else if (!r.hasChildren && r.w > 30 && r.h > 14) {
        ctx.fillStyle = fg;
        const lines = r.h > 30 ? 2 : 1;
        const top = r.y + r.h / 2 - (lines === 2 ? 7 : 0);
        ctx.fillText(fitText(ctx, r.data.name, r.w - 8), r.x + 4, top);
        if (lines === 2) {
          ctx.globalAlpha = 0.8;
          ctx.fillText(fitText(ctx, formatBytes(r.data.size), r.w - 8), r.x + 4, top + 14);
          ctx.globalAlpha = 1;
        }
      }
    });

    const outline = (i: number, color: string, width: number) => {
      const r = rects[i];
      if (!r || i === 0) return;
      ctx.strokeStyle = color;
      ctx.lineWidth = width;
      ctx.strokeRect(r.x + width / 2, r.y + width / 2, Math.max(0, r.w - width), Math.max(0, r.h - width));
    };
    if (hoverIdx > 0) outline(hoverIdx, "rgba(255,255,255,0.75)", 1);
    if (selectedId !== null) {
      const sel = rects.findIndex((r) => r.data.id === selectedId);
      if (sel > 0) outline(sel, "#ffffff", 2);
    }
  }, [rects, size, colorBy, hoverIdx, selectedId]);

  const pointAt = (e: React.MouseEvent<HTMLCanvasElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    return hitTest(rects, e.clientX - box.left, e.clientY - box.top);
  };

  const handleMove = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (!interactive) return;
    const i = pointAt(e);
    if (i === hoverIdx) return;
    setHoverIdx(i);
    const r = i > 0 ? rects[i] : undefined;
    onHover?.(
      r
        ? {
            name: r.data.name,
            pathParts: rectPath(rects, i),
            size: r.data.size,
            fileCount: r.data.fileCount,
            mtime: r.data.mtime,
            kind: r.data.kind,
            mergedCount: r.data.mergedCount,
          }
        : null,
    );
  };

  const handleClick = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (!interactive) return;
    const i = pointAt(e);
    const r = i > 0 ? rects[i] : undefined;
    onSelect?.(r?.data.id ?? null);
  };

  const handleDoubleClick = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (!interactive) return;
    const i = pointAt(e);
    if (i <= 0) return;
    const target = drillTarget(rects, i);
    if (target !== null) onDrill?.(target);
  };

  const handleContextMenu = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (!interactive) return;
    e.preventDefault();
    const i = pointAt(e);
    const r = i > 0 ? rects[i] : undefined;
    if (r && r.data.id !== null) {
      onSelect?.(r.data.id);
      onContextMenu?.(e.clientX, e.clientY, r.data.id, r.data.kind === "dir");
    }
  };

  return (
    <div ref={wrapRef} className="treemap-wrap">
      <canvas
        ref={canvasRef}
        style={{ width: size.w, height: size.h }}
        onMouseMove={handleMove}
        onMouseLeave={() => {
          setHoverIdx(-1);
          onHover?.(null);
        }}
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
        onContextMenu={handleContextMenu}
        role="img"
        aria-label="Treemap of disk usage. Use the list for keyboard navigation."
      />
    </div>
  );
}
