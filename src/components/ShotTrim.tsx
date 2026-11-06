import { convertFileSrc } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect, useRef, useState } from "react";
import { api, errMsg } from "../lib/ipc";

const THUMBS = 10;

/** Opens the trimmer for one recording in its own window. */
export function openTrim(path: string) {
  const label = `shot-trim-${Math.random().toString(36).slice(2, 8)}`;
  new WebviewWindow(label, {
    url: `index.html#/shot-trim/${encodeURIComponent(path)}`,
    title: "Regente Captura: cortar gravação",
    width: 960,
    height: 640,
    dragDropEnabled: false,
  });
}

const fmt = (t: number) => `${Math.floor(t / 60)}:${(t % 60).toFixed(1).padStart(4, "0")}`;

/** Corte da gravação: filmstrip de 10 quadros, duas alças (início e fim), FPS 1 ou 2 e a extração dos quadros. */
export default function ShotTrim({ path }: { path: string }) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const thumbRefs = useRef<(HTMLCanvasElement | null)[]>([]);
  const [dur, setDur] = useState(0);
  const [range, setRange] = useState<[number, number]>([0, 0]);
  const [fps, setFps] = useState<1 | 2>(1);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [result, setResult] = useState<{ dir: string; frames: number; contact: string } | null>(null);

  // filmstrip: one frame per tenth of the video, drawn to a canvas (drawing only, nothing is read back)
  useEffect(() => {
    const v = videoRef.current;
    if (!v || !dur) return;
    let alive = true;
    (async () => {
      for (let i = 0; i < THUMBS; i++) {
        const seeked = new Promise<void>((res) => v.addEventListener("seeked", () => res(), { once: true }));
        v.currentTime = (dur * (i + 0.5)) / THUMBS;
        await seeked;
        const c = thumbRefs.current[i];
        if (!alive) return;
        if (c && v.videoWidth) {
          c.width = 96;
          c.height = Math.max(1, Math.round((96 * v.videoHeight) / v.videoWidth));
          c.getContext("2d")?.drawImage(v, 0, 0, c.width, c.height);
        }
      }
      v.currentTime = 0;
    })().catch(() => {});
    return () => {
      alive = false;
    };
  }, [dur]);

  function move(i: 0 | 1, t: number) {
    const next: [number, number] = i === 0 ? [Math.min(t, range[1] - 0.1), range[1]] : [range[0], Math.max(t, range[0] + 0.1)];
    setRange(next);
    if (videoRef.current) videoRef.current.currentTime = next[i];
  }

  async function extract() {
    setBusy(true);
    setErr("");
    setResult(null);
    try {
      setResult(await api.shotExtract(path, range[0], range[1], fps));
    } catch (e) {
      setErr(errMsg(e));
    } finally {
      setBusy(false);
    }
  }

  /** A janela do corte não tem os painéis: a principal recebe a pasta e cola no painel ativo dela. */
  function sendToPane() {
    if (result) emitTo("main", "shot-paste", { path: result.dir }).catch((e) => setErr(errMsg(e)));
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 10, padding: 12, height: "100vh", boxSizing: "border-box" }}>
      <video ref={videoRef} src={convertFileSrc(path)} controls preload="auto" muted
        onLoadedMetadata={(e) => setDur(e.currentTarget.duration || 0)} style={{ width: "100%", maxHeight: "45vh", background: "#000" }} />
      {dur > 0 && (
        <>
          <div style={{ display: "flex", gap: 2, overflow: "hidden" }} aria-label="Quadros do vídeo">
            {Array.from({ length: THUMBS }, (_, i) => (
              <canvas key={i} ref={(el) => { thumbRefs.current[i] = el; }} style={{ flex: 1, minWidth: 0, height: 54, objectFit: "cover" }} />
            ))}
          </div>
          <label>Início {fmt(range[0])}
            <input type="range" aria-label="Início" min={0} max={dur} step={0.1} value={range[0]} onChange={(e) => move(0, Number(e.target.value))} style={{ width: "100%" }} />
          </label>
          <label>Fim {fmt(range[1])}
            <input type="range" aria-label="Fim" min={0} max={dur} step={0.1} value={range[1]} onChange={(e) => move(1, Number(e.target.value))} style={{ width: "100%" }} />
          </label>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <span>FPS</span>
            <select aria-label="FPS da extração" value={fps} onChange={(e) => setFps(Number(e.target.value) === 2 ? 2 : 1)}>
              <option value={1}>1</option>
              <option value={2}>2</option>
            </select>
            <button className="btn" disabled={busy} onClick={extract}>{busy ? "Extraindo…" : "Extrair frames"}</button>
          </div>
        </>
      )}
      {err && <div role="alert" className="meta">{err}</div>}
      {result && (
        <div style={{ display: "flex", flexDirection: "column", gap: 8, minHeight: 0, overflow: "auto" }}>
          <div>{result.frames} quadros em {result.dir}</div>
          <img src={convertFileSrc(result.contact)} alt="Folha de contato" style={{ maxWidth: "100%" }} />
          <button className="btn" onClick={sendToPane} style={{ alignSelf: "flex-start" }}>Enviar ao painel ativo</button>
        </div>
      )}
    </div>
  );
}
