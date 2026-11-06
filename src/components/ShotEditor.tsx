import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { api, errMsg } from "../lib/ipc";
import { addLayer, applyCrop, newHistory, parseDoc, redo, rectOf, setCrop, undo, type Doc, type History, type Layer, type Pt, type Rect } from "../lib/shotLayers";
import { useStore } from "../state/store";

type Tool = "arrow" | "text" | "rect" | "highlight" | "blur" | "crop";
const TOOLS: [Tool, string][] = [
  ["arrow", "Seta"],
  ["text", "Texto"],
  ["rect", "Retângulo"],
  ["highlight", "Destaque"],
  ["blur", "Blur"],
  ["crop", "Cortar"],
];
const RED = "#e53935";
const HIGHLIGHT = "rgba(255, 221, 0, 0.35)";
const AUTOSAVE_MS = 800;
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Pixelates a region of the canvas (the blur layer). */
function pixelate(ctx: CanvasRenderingContext2D, r: Rect) {
  const x = Math.max(0, Math.round(r.x));
  const y = Math.max(0, Math.round(r.y));
  const w = Math.min(ctx.canvas.width - x, Math.round(r.w));
  const h = Math.min(ctx.canvas.height - y, Math.round(r.h));
  if (w < 2 || h < 2) return;
  const k = 14;
  const small = document.createElement("canvas");
  small.width = Math.max(1, Math.ceil(w / k));
  small.height = Math.max(1, Math.ceil(h / k));
  small.getContext("2d")?.drawImage(ctx.canvas, x, y, w, h, 0, 0, small.width, small.height);
  ctx.imageSmoothingEnabled = false;
  ctx.drawImage(small, 0, 0, small.width, small.height, x, y, w, h);
  ctx.imageSmoothingEnabled = true;
}

function drawLayer(ctx: CanvasRenderingContext2D, l: Layer) {
  ctx.save();
  switch (l.type) {
    case "arrow": {
      ctx.strokeStyle = l.color;
      ctx.fillStyle = l.color;
      ctx.lineWidth = 6;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(l.from.x, l.from.y);
      ctx.lineTo(l.to.x, l.to.y);
      ctx.stroke();
      const ang = Math.atan2(l.to.y - l.from.y, l.to.x - l.from.x);
      const head = 22;
      ctx.beginPath();
      ctx.moveTo(l.to.x, l.to.y);
      ctx.lineTo(l.to.x - head * Math.cos(ang - 0.45), l.to.y - head * Math.sin(ang - 0.45));
      ctx.lineTo(l.to.x - head * Math.cos(ang + 0.45), l.to.y - head * Math.sin(ang + 0.45));
      ctx.closePath();
      ctx.fill();
      break;
    }
    case "text":
      ctx.fillStyle = l.color;
      ctx.font = "bold 28px 'Segoe UI', sans-serif";
      ctx.textBaseline = "top";
      ctx.fillText(l.text, l.at.x, l.at.y);
      break;
    case "rect":
      ctx.strokeStyle = l.color;
      ctx.lineWidth = 5;
      ctx.strokeRect(l.rect.x, l.rect.y, l.rect.w, l.rect.h);
      break;
    case "highlight":
      ctx.fillStyle = l.color;
      ctx.fillRect(l.rect.x, l.rect.y, l.rect.w, l.rect.h);
      break;
    case "blur":
      pixelate(ctx, l.rect);
      break;
  }
  ctx.restore();
}

/** The document drawn on a canvas: the crop of the image, then the layers (and the one being drawn). */
function draw(c: HTMLCanvasElement, img: HTMLImageElement, doc: Doc, draft: Layer | null) {
  const crop = doc.crop ?? { x: 0, y: 0, w: img.naturalWidth, h: img.naturalHeight };
  c.width = crop.w;
  c.height = crop.h;
  const ctx = c.getContext("2d");
  if (!ctx) return;
  ctx.drawImage(img, crop.x, crop.y, crop.w, crop.h, 0, 0, crop.w, crop.h);
  for (const l of applyCrop(draft ? [...doc.layers, draft] : doc.layers, crop)) drawLayer(ctx, l);
}

/** The layer a drag makes (crop returns only its preview: a white box, never a layer). */
function layerFor(tool: Tool, a: Pt, b: Pt): Layer | null {
  const r = rectOf(a, b);
  switch (tool) {
    case "arrow":
      return Math.hypot(b.x - a.x, b.y - a.y) >= 6 ? { type: "arrow", from: a, to: b, color: RED } : null;
    case "rect":
      return r.w >= 3 && r.h >= 3 ? { type: "rect", rect: r, color: RED } : null;
    case "highlight":
      return r.w >= 3 && r.h >= 3 ? { type: "highlight", rect: r, color: HIGHLIGHT } : null;
    case "blur":
      return r.w >= 3 && r.h >= 3 ? { type: "blur", rect: r } : null;
    case "crop":
      return { type: "rect", rect: r, color: "#ffffff" };
    default:
      return null;
  }
}

/** Keeps a rectangle inside a W×H image. */
function clampTo(r: Rect, W: number, H: number): Rect {
  const x = Math.max(0, Math.min(r.x, W));
  const y = Math.max(0, Math.min(r.y, H));
  return { x, y, w: Math.max(0, Math.min(r.w, W - x)), h: Math.max(0, Math.min(r.h, H - y)) };
}

async function base64Of(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** Editor de capturas (Shot): camadas sobre a imagem, recorte, desfazer/refazer e autosave. */
export default function ShotEditor({ path }: { path: string }) {
  const [img, setImg] = useState<HTMLImageElement | null>(null);
  const [hist, setHist] = useState<History>(newHistory());
  const [tool, setTool] = useState<Tool>("arrow");
  const [draft, setDraft] = useState<{ tool: Tool; a: Pt; b: Pt } | null>(null);
  const [text, setText] = useState<{ at: Pt; value: string } | null>(null);
  const [status, setStatus] = useState("Carregando…");
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const savedDoc = useRef<Doc | null>(null);
  const doc = hist.present;
  const view = img ? doc.crop ?? { x: 0, y: 0, w: img.naturalWidth, h: img.naturalHeight } : null;
  const draftLayer = draft ? layerFor(draft.tool, draft.a, draft.b) : null;

  /** Saves queue behind each other: an autosave never overlaps the previous one (or the flush on close). */
  const inflight = useRef<Promise<void>>(Promise.resolve());

  function persist(d: Doc, im: HTMLImageElement): Promise<void> {
    inflight.current = inflight.current.then(() => save(d, im));
    return inflight.current;
  }

  async function save(d: Doc, im: HTMLImageElement) {
    if (d === savedDoc.current) return;
    setStatus("Salvando…");
    try {
      const c = document.createElement("canvas");
      draw(c, im, d, null);
      const blob = await new Promise<Blob | null>((res) => c.toBlob(res, "image/png"));
      if (!blob) throw new Error("não foi possível gerar a imagem");
      await api.shotSaveEdit(path, await base64Of(blob), JSON.stringify({ crop: d.crop, layers: d.layers }));
      savedDoc.current = d;
      setStatus("Salvo");
    } catch (e) {
      setStatus(errMsg(e));
    }
  }

  // load: the original image (from memory, so the canvas stays readable) and the saved document
  useEffect(() => {
    let url = "";
    let alive = true;
    Promise.all([api.shotEditGet(path), api.shotRead(path)])
      .then(async ([state, buf]) => {
        url = URL.createObjectURL(new Blob([buf], { type: "image/png" }));
        const im = new Image();
        im.src = url;
        await im.decode();
        if (!alive) return;
        const initial = parseDoc(state.doc);
        savedDoc.current = initial;
        setHist(newHistory(initial));
        setImg(im);
        setStatus("Pronto");
      })
      .catch((e) => alive && setStatus(errMsg(e)));
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [path]);

  // the canvas shows the document (and the shape being dragged)
  useEffect(() => {
    if (img && canvasRef.current) draw(canvasRef.current, img, doc, draftLayer);
  });

  // autosave, debounced; the latest state is flushed when the editor closes
  useEffect(() => {
    if (!img || doc === savedDoc.current) return;
    const t = window.setTimeout(() => void persist(doc, img), AUTOSAVE_MS);
    return () => window.clearTimeout(t);
  }, [img, doc]);
  const latest = useRef({ doc, img });
  latest.current = { doc, img };
  useEffect(
    () => () => {
      const l = latest.current;
      if (l.img && l.doc !== savedDoc.current) void persist(l.doc, l.img);
    },
    [],
  );

  // Ctrl+Z / Ctrl+Y (Ctrl+Shift+Z also redoes); typing in the text box keeps its own undo
  useEffect(() => {
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.target instanceof HTMLInputElement) return;
      const mod = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (mod && k === "z") {
        e.preventDefault();
        setHist((h) => (e.shiftKey ? redo(h) : undo(h)));
      } else if (mod && k === "y") {
        e.preventDefault();
        setHist(redo);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  /** A canvas point in image pixels: the view is scaled to fit the window, and offset by the crop. */
  function at(e: PointerEvent<HTMLCanvasElement>): Pt {
    const c = canvasRef.current;
    const o = doc.crop ?? { x: 0, y: 0 };
    if (!c) return { x: 0, y: 0 };
    const r = c.getBoundingClientRect();
    return { x: Math.round(o.x + ((e.clientX - r.left) * c.width) / r.width), y: Math.round(o.y + ((e.clientY - r.top) * c.height) / r.height) };
  }

  function down(e: PointerEvent<HTMLCanvasElement>) {
    if (!img) return;
    const p = at(e);
    if (tool === "text") {
      setText({ at: p, value: "" });
      return;
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    setDraft({ tool, a: p, b: p });
  }

  function move(e: PointerEvent<HTMLCanvasElement>) {
    if (draft) setDraft({ ...draft, b: at(e) });
  }

  function up() {
    if (!draft || !img) return setDraft(null);
    const { tool: t, a, b } = draft;
    setDraft(null);
    if (t === "crop") {
      const r = clampTo(rectOf(a, b), img.naturalWidth, img.naturalHeight);
      if (r.w >= 4 && r.h >= 4) setHist((h) => setCrop(h, r));
      return;
    }
    const l = layerFor(t, a, b);
    if (l) setHist((h) => addLayer(h, l));
  }

  function commitText() {
    const t = text;
    setText(null);
    if (t && t.value.trim()) setHist((h) => addLayer(h, { type: "text", at: t.at, text: t.value.trim(), color: RED }));
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100vh" }}>
      <div role="toolbar" aria-label="Ferramentas" style={{ display: "flex", gap: 6, flexWrap: "wrap", padding: 8, alignItems: "center" }}>
        {TOOLS.map(([id, label]) => (
          <button key={id} className={`tb-btn ${tool === id ? "on" : ""}`} aria-pressed={tool === id} onClick={() => setTool(id)}>
            {label}
          </button>
        ))}
        <span style={{ flex: 1 }} />
        <button className="tb-btn" disabled={!hist.past.length} onClick={() => setHist(undo)}>Desfazer</button>
        <button className="tb-btn" disabled={!hist.future.length} onClick={() => setHist(redo)}>Refazer</button>
        {doc.crop && <button className="tb-btn" onClick={() => setHist((h) => setCrop(h, null))}>Remover recorte</button>}
        <button className="tb-btn" onClick={() => revealItemInDir(path).catch(toast)}>Abrir no Explorer</button>
        <span className="meta" role="status">{status}</span>
      </div>
      <div style={{ flex: 1, overflow: "auto", display: "flex", justifyContent: "center", padding: 8 }}>
        {img && view ? (
          <div style={{ position: "relative", display: "inline-block", maxWidth: "100%" }}>
            <canvas ref={canvasRef} onPointerDown={down} onPointerMove={move} onPointerUp={up}
              style={{ display: "block", maxWidth: "100%", maxHeight: "calc(100vh - 64px)", cursor: tool === "text" ? "text" : "crosshair", touchAction: "none" }} />
            {text && (
              <input autoFocus aria-label="Texto da anotação" value={text.value}
                onChange={(e) => setText({ ...text, value: e.target.value })}
                onBlur={commitText}
                onKeyDown={(e: KeyboardEvent) => {
                  if (e.key === "Enter") commitText();
                  if (e.key === "Escape") setText(null);
                }}
                style={{
                  position: "absolute",
                  left: `${((text.at.x - view.x) / view.w) * 100}%`,
                  top: `${((text.at.y - view.y) / view.h) * 100}%`,
                  font: "bold 20px 'Segoe UI', sans-serif",
                  color: RED,
                  background: "rgba(255,255,255,0.85)",
                  border: "1px dashed " + RED,
                }} />
            )}
          </div>
        ) : (
          <div className="meta">{status}</div>
        )}
      </div>
    </div>
  );
}
