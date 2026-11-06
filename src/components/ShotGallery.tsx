import { convertFileSrc } from "@tauri-apps/api/core";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { ask } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api, errMsg, type ShotInfo } from "../lib/ipc";
import { startPointerDrag } from "../lib/pointerDrag";
import { useStore } from "../state/store";
import { openTrim } from "./ShotTrim";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const BADGE: Record<ShotInfo["kind"], string> = { png: "imagem", mp4: "vídeo", frames: "frames" };

/** Opens the editor for one capture in its own window. */
function openEditor(path: string) {
  const label = `shot-editor-${Math.random().toString(36).slice(2, 8)}`;
  new WebviewWindow(label, {
    url: `index.html#/shot-editor/${encodeURIComponent(path)}`,
    title: "Regente Captura: editor",
    width: 1100,
    height: 760,
    dragDropEnabled: false,
  });
}

/** Galeria das capturas: miniaturas, editar, copiar o caminho, mostrar no Explorer e excluir. Arrastar uma captura
 *  para um painel de terminal cola o caminho. */
export default function ShotGallery() {
  const [items, setItems] = useState<ShotInfo[] | null>(null);
  const [ffmpeg, setFfmpeg] = useState(false);

  function load() {
    api.shotList().then(setItems).catch(toast);
  }

  useEffect(() => {
    load();
    api.shotGet().then((r) => setFfmpeg(r.ffmpeg)).catch(() => {});
    // files may be deleted outside the app: re-list whenever the window gets the focus back
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, []);

  async function remove(item: ShotInfo) {
    const ok = await ask(`Mover ${item.name} para a Lixeira?`, { title: "Regente Captura", kind: "warning" });
    if (!ok) return;
    try {
      await api.shotDelete(item.path);
      load();
    } catch (e) {
      toast(e);
    }
  }

  async function copyPath(item: ShotInfo) {
    try {
      await navigator.clipboard.writeText(item.path);
      useStore.getState().toast("Caminho copiado", "ok");
    } catch (e) {
      toast(e);
    }
  }

  const bar = (
    <div className="row" style={{ gap: 6, marginBottom: 8, flexWrap: "wrap" }}>
      <button className="btn" onClick={load}>Atualizar</button>
      <button className="btn" onClick={() => api.shotRegionBegin().catch(toast)}>Capturar região</button>
      <button className="btn" disabled={!ffmpeg} title={ffmpeg ? "Gravar uma região da tela" : "instale o ffmpeg (winget install Gyan.FFmpeg)"} onClick={() => api.shotRecordBegin().catch(toast)}>
        Gravar região
      </button>
    </div>
  );
  if (!items) return <div className="meta">Carregando…</div>;
  if (!items.length) return <>{bar}<div className="meta">Nenhuma captura ainda.</div></>;
  return (
    <>
    {bar}
    <div className="shot-grid">
      {items.map((item) => (
        <div key={item.path} className="shot-card" onPointerDown={(e) => { startPointerDrag(e, { kind: "shot-path", data: item.path }, item.name); }}>
          <div className="shot-thumb">
            {item.kind === "png" ? <img src={convertFileSrc(item.path)} alt={item.name} loading="lazy" draggable={false} /> : <span className="meta">{BADGE[item.kind]}</span>}
          </div>
          <div className="meta shot-name" title={item.name}>{item.name}</div>
          <div className="row" style={{ gap: 4, flexWrap: "wrap" }}>
            <span className="kbd">{BADGE[item.kind]}</span>
            {item.kind === "png" && <button className="btn" onClick={() => openEditor(item.path)}>Editar</button>}
            {item.kind === "mp4" && <button className="btn" onClick={() => openTrim(item.path)}>Cortar</button>}
            <button className="btn" onClick={() => copyPath(item)}>Copiar caminho</button>
            <button className="btn" onClick={() => revealItemInDir(item.path).catch(toast)}>Mostrar</button>
            <button className="btn" onClick={() => remove(item)}>Excluir</button>
          </div>
        </div>
      ))}
    </div>
    </>
  );
}
