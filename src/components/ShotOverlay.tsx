import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { api, errMsg } from "../lib/ipc";
import { rectOf, toImageRect, type Pt, type Rect } from "../lib/shotLayers";

/** Overlay de região (Shot): a tela congelada; arrastar escolhe o retângulo; soltar ou Enter confirma; Esc cancela. */
export default function ShotOverlay() {
  const [src, setSrc] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ a: Pt; b: Pt } | null>(null);
  const [err, setErr] = useState("");
  const imgRef = useRef<HTMLImageElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const sel: Rect | null = drag ? rectOf(drag.a, drag.b) : null;

  useEffect(() => {
    let url = "";
    let alive = true;
    api
      .shotOverlayImage()
      .then((buf) => {
        if (!alive) return;
        url = URL.createObjectURL(new Blob([buf], { type: "image/png" }));
        setSrc(url);
      })
      .catch((e) => alive && setErr(errMsg(e)));
    rootRef.current?.focus();
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, []);

  /** The selection in image pixels: the screen can be scaled (HiDPI), so the image is the source of truth. */
  function pick(r: Rect): Rect | null {
    const img = imgRef.current;
    if (!img) return null;
    const p = toImageRect(r, window.innerWidth, window.innerHeight, img.naturalWidth, img.naturalHeight);
    return p.w >= 2 && p.h >= 2 ? p : null;
  }

  async function commit(r: Rect) {
    const p = pick(r);
    if (!p) return setDrag(null);
    try {
      await api.shotRegionCommit(p.x, p.y, p.w, p.h);
    } catch (e) {
      setErr(errMsg(e));
      setDrag(null);
    }
  }

  function cancel() {
    api.shotRegionCancel().catch(() => {});
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    } else if (e.key === "Enter" && sel) {
      e.preventDefault();
      void commit(sel);
    }
  }

  function down(e: PointerEvent<HTMLDivElement>) {
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = { x: e.clientX, y: e.clientY };
    setDrag({ a: p, b: p });
  }

  function move(e: PointerEvent<HTMLDivElement>) {
    if (drag) setDrag({ a: drag.a, b: { x: e.clientX, y: e.clientY } });
  }

  function up() {
    if (sel) void commit(sel);
    else setDrag(null);
  }

  const px = sel ? pick(sel) : null;
  return (
    <div ref={rootRef} tabIndex={-1} onKeyDown={onKey} onPointerDown={down} onPointerMove={move} onPointerUp={up}
      style={{ position: "fixed", inset: 0, cursor: "crosshair", background: "#000", overflow: "hidden", outline: "none", userSelect: "none" }}>
      {src && <img ref={imgRef} src={src} alt="" draggable={false} style={{ position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }} />}
      {sel && (
        <div style={{ position: "absolute", left: sel.x, top: sel.y, width: sel.w, height: sel.h, border: "1px solid #fff", boxShadow: "0 0 0 100vmax rgba(0,0,0,0.45)", pointerEvents: "none" }}>
          {px && <span style={{ position: "absolute", left: 0, top: -22, padding: "2px 6px", background: "#000", color: "#fff", font: "12px monospace" }}>{px.w}×{px.h}</span>}
        </div>
      )}
      <div role="status" style={{ position: "absolute", left: 12, bottom: 12, padding: "6px 10px", background: "rgba(0,0,0,0.7)", color: "#fff", font: "13px sans-serif", pointerEvents: "none" }}>
        {err || "Arraste para escolher a região · Enter ou soltar confirma · Esc cancela"}
      </div>
    </div>
  );
}
