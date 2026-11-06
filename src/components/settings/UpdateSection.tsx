import { useEffect, useState } from "react";
import { api, errMsg, type UpdateStatus } from "../../lib/ipc";
import { progressPercent, statusText } from "../../lib/update";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Atualização automática: só usa a rede por clique. Canal vazio = sem canal configurado. */
export default function UpdateSection() {
  const [current, setCurrent] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [st, setSt] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [percent, setPercent] = useState<number | null>(null);
  const [downloaded, setDownloaded] = useState(false);

  useEffect(() => {
    api.appInfo().then((i) => setCurrent(i.version)).catch(toast);
    api.settingsGet().then((s) => setEndpoint(s.updateEndpoint)).catch(toast);
  }, []);

  async function saveChannel() {
    try {
      const s = await api.settingsGet();
      await api.settingsSet({ ...s, updateEndpoint: endpoint.trim() });
      setSt(null);
      useStore.getState().toast("Canal salvo", "ok");
    } catch (e) {
      toast(e);
    }
  }

  async function check() {
    setBusy(true);
    setDownloaded(false);
    try {
      setSt(await api.updateCheck());
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  async function install() {
    setBusy(true);
    setDownloaded(false);
    setPercent(0);
    let total: number | null = null;
    let got = 0;
    try {
      await api.updateInstall((e) => {
        if (e.event === "started") total = e.data.contentLength;
        if (e.event === "progress") {
          got += e.data.chunk;
          setPercent(progressPercent(got, total));
        }
        if (e.event === "finished") setDownloaded(true);
      });
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h3>Atualizações</h3>
      <div className="meta" style={{ marginBottom: 8 }}>Versão atual: {current || "…"}</div>
      <div className="row" style={{ marginBottom: 8 }}>
        <span className="meta" style={{ width: 150 }}>Canal (URL do latest.json)</span>
        <input aria-label="Canal de atualização" value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder="https://…/latest.json" />
        <button className="btn" onClick={saveChannel}>Salvar canal</button>
      </div>
      <div className="row" style={{ marginBottom: 8 }}>
        <button className="btn" disabled={busy} onClick={check}>Procurar agora</button>
        <span className="meta">Nada é baixado sem o seu clique.</span>
      </div>
      {st && <p style={{ marginBottom: 8 }}>{statusText(st)}</p>}
      {st?.state === "disponivel" && (
        <>
          {st.notes && <pre style={{ whiteSpace: "pre-wrap", marginBottom: 8 }}>{st.notes}</pre>}
          <button className="btn" disabled={busy} onClick={install}>Baixar e instalar</button>
        </>
      )}
      {busy && percent !== null && <div className="meta" style={{ marginTop: 8 }}>Baixando… {percent}%</div>}
      {downloaded && <div className="meta" style={{ marginTop: 8 }}>Download concluído. Instalando: no Windows o Regente reinicia sozinho ao terminar.</div>}
    </>
  );
}
