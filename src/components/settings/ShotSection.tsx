import { useEffect, useState } from "react";
import { api, errMsg, type ShotConfig } from "../../lib/ipc";
import { useStore } from "../../state/store";
import ShotGallery from "../ShotGallery";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

export default function ShotSection() {
  const [cfg, setCfg] = useState<ShotConfig | null>(null);
  const [dir, setDir] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.shotGet().then((r) => { setCfg(r.config); setDir(r.dir); }).catch(toast);
  }, []);

  if (!cfg) return <><h3>Captura de tela</h3><div className="meta">Carregando…</div></>;

  const set = (p: Partial<ShotConfig>) => setCfg({ ...cfg, ...p });

  async function save() {
    if (!cfg || busy) return;
    setBusy(true);
    try {
      await api.shotSave(cfg);
      useStore.getState().toast("Captura de tela salva", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  async function full() {
    try {
      await api.shotFull();
    } catch (e) {
      toast(e);
    }
  }

  return (
    <>
      <h3>Captura de tela</h3>
      <div className="meta" style={{ marginBottom: 8 }}>Salva em {dir}</div>
      <label className="row" style={{ marginBottom: 8 }}>
        <input type="checkbox" checked={cfg.hotkeys} onChange={(e) => set({ hotkeys: e.target.checked })} /> Atalhos globais de captura (desligados por padrão)
      </label>
      <div className="row" style={{ marginBottom: 6 }}>
        <span className="meta" style={{ width: 150 }}>Tela inteira</span>
        <input aria-label="Atalho da tela inteira" value={cfg.fullKey} onChange={(e) => set({ fullKey: e.target.value })} />
      </div>
      <div className="row" style={{ marginBottom: 6 }}>
        <span className="meta" style={{ width: 150 }}>Região</span>
        <input aria-label="Atalho da região" value={cfg.regionKey} onChange={(e) => set({ regionKey: e.target.value })} />
      </div>
      <div className="row" style={{ marginBottom: 10 }}>
        <span className="meta" style={{ width: 150 }}>Gravar região</span>
        <input aria-label="Atalho de gravar região" value={cfg.recordKey} onChange={(e) => set({ recordKey: e.target.value })} />
      </div>
      <label className="row" style={{ marginBottom: 10 }}>
        <input type="checkbox" checked={cfg.agentCapture} onChange={(e) => set({ agentCapture: e.target.checked })} /> Permitir que o agente capture a tela (ferramenta shot_capture)
      </label>
      <div className="row" style={{ marginBottom: 14 }}>
        <button className="btn" disabled={busy} onClick={save}>Salvar</button>
        <button className="btn" onClick={full}>Capturar tela agora</button>
      </div>
      <h4>Galeria</h4>
      <ShotGallery />
    </>
  );
}
