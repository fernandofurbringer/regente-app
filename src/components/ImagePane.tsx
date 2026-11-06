import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { PaneRow } from "../lib/ipc";

export default function ImagePane({ pane }: { pane: PaneRow }) {
  const [v, setV] = useState(0);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    const onReload = (e: Event) => { if ((e as CustomEvent).detail === pane.id) { setFailed(false); setV((x) => x + 1); } };
    window.addEventListener("regente:reload-image", onReload);
    return () => window.removeEventListener("regente:reload-image", onReload);
  }, [pane.id]);
  const src = `${convertFileSrc(pane.urlOrPath ?? "")}?v=${v}`;
  return (
    <div className="img-pane">
      {failed ? (
        <div className="empty">
          Não foi possível carregar a imagem.{" "}
          <button className="btn" onClick={() => { setFailed(false); setV(v + 1); }}>Reabrir a imagem</button>
        </div>
      ) : (
        <img src={src} alt={pane.name} onError={() => setFailed(true)} />
      )}
      <button className="icon-btn" style={{ position: "absolute", top: 6, right: 6 }} aria-label="Recarregar imagem" onClick={() => setV(v + 1)}>⟳</button>
    </div>
  );
}
