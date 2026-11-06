//! Editor de capturas (F3): camadas, recorte e histórico de desfazer/refazer. Puro e testado; o desenho fica no editor.
//! Coordenadas das camadas são da imagem original. O recorte (`crop`) é um retângulo nessa mesma imagem.

export interface Pt {
  x: number;
  y: number;
}
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}
export type Layer =
  | { type: "arrow"; from: Pt; to: Pt; color: string }
  | { type: "text"; at: Pt; text: string; color: string }
  | { type: "rect"; rect: Rect; color: string }
  | { type: "highlight"; rect: Rect; color: string }
  | { type: "blur"; rect: Rect };
export interface Doc {
  crop: Rect | null;
  layers: Layer[];
}
export interface History {
  past: Doc[];
  present: Doc;
  future: Doc[];
}

export const EMPTY_DOC: Doc = { crop: null, layers: [] };
const LIMIT = 200;

export const newHistory = (doc: Doc = EMPTY_DOC): History => ({ past: [], present: doc, future: [] });

/** A new state becomes the undo point; the redo stack goes away. */
export function push(h: History, doc: Doc): History {
  return { past: [...h.past, h.present].slice(-LIMIT), present: doc, future: [] };
}

export function addLayer(h: History, layer: Layer): History {
  return push(h, { ...h.present, layers: [...h.present.layers, layer] });
}

export function setCrop(h: History, crop: Rect | null): History {
  return push(h, { ...h.present, crop });
}

export function undo(h: History): History {
  const prev = h.past[h.past.length - 1];
  if (!prev) return h;
  return { past: h.past.slice(0, -1), present: prev, future: [h.present, ...h.future] };
}

export function redo(h: History): History {
  const next = h.future[0];
  if (!next) return h;
  return { past: [...h.past, h.present], present: next, future: h.future.slice(1) };
}

/** Two corners, in any order, as a rectangle with positive sides. */
export function rectOf(a: Pt, b: Pt): Rect {
  return { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), w: Math.abs(b.x - a.x), h: Math.abs(b.y - a.y) };
}

/** A selection in CSS pixels, turned into image pixels and clamped to the image. */
export function toImageRect(sel: Rect, viewW: number, viewH: number, imgW: number, imgH: number): Rect {
  const sx = imgW / viewW;
  const sy = imgH / viewH;
  const x = Math.max(0, Math.round(sel.x * sx));
  const y = Math.max(0, Math.round(sel.y * sy));
  return { x, y, w: Math.max(0, Math.min(imgW - x, Math.round(sel.w * sx))), h: Math.max(0, Math.min(imgH - y, Math.round(sel.h * sy))) };
}

function shift(l: Layer, dx: number, dy: number): Layer {
  switch (l.type) {
    case "arrow":
      return { ...l, from: { x: l.from.x + dx, y: l.from.y + dy }, to: { x: l.to.x + dx, y: l.to.y + dy } };
    case "text":
      return { ...l, at: { x: l.at.x + dx, y: l.at.y + dy } };
    default:
      return { ...l, rect: { ...l.rect, x: l.rect.x + dx, y: l.rect.y + dy } };
  }
}

/** The box a layer covers (text is estimated from its length). */
function boundsOf(l: Layer): Rect {
  switch (l.type) {
    case "arrow":
      return rectOf(l.from, l.to);
    case "text":
      return { x: l.at.x, y: l.at.y, w: Math.max(1, l.text.length * 14), h: 32 };
    default:
      return l.rect;
  }
}

function touches(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/** The layers as seen through a crop: moved into the crop's origin, and the ones outside it are dropped. */
export function applyCrop(layers: Layer[], crop: Rect): Layer[] {
  const view: Rect = { x: 0, y: 0, w: crop.w, h: crop.h };
  return layers.map((l) => shift(l, -crop.x, -crop.y)).filter((l) => touches(boundsOf(l), view));
}

/** The sidecar (`<file>.shot.json`) read back safely: anything malformed opens as an empty document. */
export function parseDoc(text: string | null): Doc {
  if (!text) return EMPTY_DOC;
  try {
    const v = JSON.parse(text);
    const isRect = (r: unknown): r is Rect => !!r && typeof r === "object" && ["x", "y", "w", "h"].every((k) => typeof (r as Record<string, unknown>)[k] === "number");
    const layers = Array.isArray(v?.layers) ? (v.layers as Layer[]) : [];
    return { crop: isRect(v?.crop) ? v.crop : null, layers };
  } catch {
    return EMPTY_DOC;
  }
}
