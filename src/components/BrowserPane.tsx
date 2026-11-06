import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { errMsg, type PaneRow } from "../lib/ipc";
import { useStore } from "../state/store";

/** Só http(s); host sem esquema (localhost:3000, exemplo.com) vira http://. Outro esquema → null. */
function normalize(u: string): string | null {
  const v = u.trim();
  if (/^https?:\/\//i.test(v)) return v;
  if (/^[a-z][a-z\d+.-]*:(?!\d)/i.test(v)) return null;
  return `http://${v}`;
}

// ponytail: iframe — sites com X-Frame-Options não carregam (localhost carrega). Trocar por webview filho do Tauri se precisar de sites externos.
export default function BrowserPane({ pane }: { pane: PaneRow }) {
  const [url, setUrl] = useState(pane.urlOrPath ?? "http://localhost:3000");
  const [src, setSrc] = useState(url);
  const [k, setK] = useState(0);
  const toast = useStore((s) => s.toast);
  function go() {
    const u = normalize(url);
    if (!u) return toast("Só endereços http:// ou https://", "erro");
    setUrl(u);
    setSrc(u);
    setK(k + 1);
  }
  return (
    <div style={{ position: "absolute", inset: 0, display: "flex", flexDirection: "column" }}>
      <div className="browser-bar">
        <button className="icon-btn" aria-label="Recarregar" onClick={() => setK(k + 1)}>⟳</button>
        <input aria-label="Endereço" value={url} onChange={(e) => setUrl(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") go(); }} />
        <button className="icon-btn" aria-label="Abrir no navegador do sistema" onClick={() => openUrl(src).catch((e) => toast(errMsg(e), "erro"))}>↗</button>
      </div>
      <iframe key={k} className="browser-frame" src={src} title={pane.name} style={{ flex: 1 }}
        sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-downloads" />
    </div>
  );
}
