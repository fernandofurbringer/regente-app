import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { api, errMsg } from "../../lib/ipc";
import { useStore } from "../../state/store";

export default function LogsSection() {
  const [busy, setBusy] = useState(false);
  const [zip, setZip] = useState<string | null>(null);
  const [dataDir, setDataDir] = useState("");
  const toast = useStore((s) => s.toast);
  useEffect(() => { api.appInfo().then((i) => setDataDir(i.dataDir)).catch(() => {}); }, []);
  async function gen() {
    setBusy(true);
    try {
      setZip(await api.diagGenerate());
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <h3>Logs e diagnóstico</h3>
      <p>O diagnóstico junta logs e configurações, sem senhas, chaves ou e-mails.</p>
      <div className="row">
        <button className="btn primary" disabled={busy} onClick={gen}>{busy ? "Gerando…" : "Gerar diagnóstico"}</button>
        <button className="btn" disabled={!dataDir} onClick={() => openPath(`${dataDir}\\logs`).catch((e) => toast(errMsg(e), "erro"))}>Abrir pasta de logs</button>
      </div>
      {zip && (
        <div className="card" role="status">
          <strong>Diagnóstico pronto</strong>
          <div style={{ fontFamily: "var(--mono)", fontSize: 12 }}>{zip}</div>
          <div className="actions">
            <button className="btn" onClick={() => revealItemInDir(zip).catch((e) => toast(errMsg(e), "erro"))}>Mostrar na pasta</button>
            <button className="btn" onClick={() => navigator.clipboard.writeText(zip).then(() => toast("Caminho copiado", "ok"), (e) => toast(errMsg(e), "erro"))}>Copiar caminho</button>
          </div>
        </div>
      )}
    </>
  );
}
