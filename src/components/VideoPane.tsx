import { useEffect, useRef, useState } from "react";
import type { PaneRow } from "../lib/ipc";
import { isHttpsUrl, videoSrc } from "../lib/media";
import { openUrl } from "@tauri-apps/plugin-opener";

/** Posição e play/pause de cada vídeo, por painel. Fica na sessão: reconectar o painel retoma de onde parou. */
interface Saved { t: number; paused: boolean }
const keyOf = (id: string) => `regente:video:${id}`;

function readSaved(id: string): Saved | null {
  try {
    const v = JSON.parse(sessionStorage.getItem(keyOf(id)) ?? "null") as Saved | null;
    return v && Number.isFinite(v.t) ? v : null;
  } catch {
    return null;
  }
}

function writeSaved(id: string, v: Saved): void {
  try {
    sessionStorage.setItem(keyOf(id), JSON.stringify(v));
  } catch {
    // sessionStorage bloqueado: o vídeo segue sem lembrar a posição
  }
}

export default function VideoPane({ pane }: { pane: PaneRow }) {
  const ref = useRef<HTMLVideoElement>(null);
  const [failed, setFailed] = useState(false);
  const src = videoSrc(pane.urlOrPath ?? "");

  useEffect(() => {
    const v = ref.current;
    if (!v || !src) return;
    const save = () => writeSaved(pane.id, { t: v.currentTime, paused: v.paused });
    const restore = () => {
      const s = readSaved(pane.id);
      if (!s) return;
      v.currentTime = s.t;
      if (!s.paused) v.play().catch(() => {}); // o autoplay do WebView pode recusar: fica pausado
    };
    v.addEventListener("loadedmetadata", restore);
    v.addEventListener("timeupdate", save);
    v.addEventListener("pause", save);
    v.addEventListener("play", save);
    return () => {
      save();
      v.removeEventListener("loadedmetadata", restore);
      v.removeEventListener("timeupdate", save);
      v.removeEventListener("pause", save);
      v.removeEventListener("play", save);
    };
  }, [pane.id, src]);

  if (isHttpsUrl(pane.urlOrPath ?? "")) {
    // a fonte remota não toca no painel: abre no navegador
    return <div className="empty">Vídeo remoto: <button className="btn" onClick={() => openUrl(pane.urlOrPath!).catch(() => {})}>Abrir no navegador</button></div>;
  }
  if (!src) return <div className="empty">Este vídeo não tem um caminho local válido.</div>;
  if (failed) return <div className="empty">Não foi possível tocar o vídeo: {pane.urlOrPath}</div>;
  return (
    <div className="video-pane">
      <video ref={ref} src={src} controls preload="metadata" aria-label={pane.name} onError={() => setFailed(true)} />
    </div>
  );
}
