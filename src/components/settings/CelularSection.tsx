import { useEffect, useState } from "react";
import { api, errMsg, type MobileStatus } from "../../lib/ipc";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Servidor do celular: desligado por padrão; liga só quando o usuário pede. */
export default function CelularSection() {
  const [st, setSt] = useState<MobileStatus | null>(null);
  const [httpOnly, setHttpOnly] = useState(false);
  const [qr, setQr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = () => api.mobileStatus().then(setSt).catch(toast);

  // o código rotaciona a cada 5 min: a tela acompanha enquanto estiver aberta
  useEffect(() => {
    load();
    const id = window.setInterval(load, 3000);
    return () => window.clearInterval(id);
  }, []);

  if (!st) return <><h3>Celular</h3><div className="meta">Carregando…</div></>;

  async function start() {
    setBusy(true);
    try {
      setSt(await api.mobileStart(httpOnly));
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    try {
      await api.mobileStop();
      setQr(null);
      await load();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  async function showCode() {
    try {
      await api.mobileShowCode();
      await load();
    } catch (e) {
      toast(e);
    }
  }

  async function makeQr() {
    try {
      setQr(await api.mobilePairQr());
    } catch (e) {
      toast(e);
    }
  }

  async function revoke(id: string) {
    try {
      await api.mobileRevoke(id);
      await load();
    } catch (e) {
      toast(e);
    }
  }

  return (
    <>
      <h3>Celular</h3>
      <p className="meta" style={{ marginBottom: 8 }}>
        Controle os painéis pelo celular na sua rede local. Desligado por padrão. Ao ligar, o Windows pode pedir permissão de rede: a decisão é sua.
        O celular não tem acesso a arquivos, não abre painel em modo danger e não envia nada para fora da sua rede.
      </p>
      <p className="meta" style={{ marginBottom: 8 }}>
        Avisos: com a página do celular aberta, o navegador mostra "agente terminou", "perguntou" ou "gate" (só o tipo e o painel, sem texto).
        Web Push, que avisa com a página fechada, não é suportado com certificado autoassinado.
      </p>

      {!st.running && (
        <>
          <label className="row" style={{ marginBottom: 8 }}>
            <input type="checkbox" checked={httpOnly} onChange={(e) => setHttpOnly(e.target.checked)} /> Apenas HTTP (só neste computador, para testes)
          </label>
          <div className="row" style={{ marginBottom: 14 }}>
            <button className="btn" disabled={busy} onClick={start}>Ligar celular</button>
          </div>
        </>
      )}

      {st.running && (
        <>
          <div className="row" style={{ marginBottom: 6 }}>
            <span className="meta" style={{ width: 150 }}>Endereço</span>
            <code aria-label="Endereço do celular">{st.url}</code>
          </div>
          <div className="row" style={{ marginBottom: 6 }}>
            <span className="meta" style={{ width: 150 }}>Código</span>
            {st.code ? (
              <>
                <code aria-label="Código de pareamento" style={{ fontSize: "1.2rem" }}>{st.code}</code>
                <span className="meta">válido por 5 min, uso único; aprovação aqui é necessária</span>
              </>
            ) : (
              <button className="btn" disabled={busy} onClick={showCode}>Mostrar código</button>
            )}
          </div>
          {st.fingerprint && (
            <div className="card" style={{ marginBottom: 10, padding: 10, border: "1px solid var(--line, #888)", borderRadius: 8 }}>
              <div className="meta" style={{ marginBottom: 4 }}>Impressão digital do certificado (SHA-256)</div>
              <code style={{ wordBreak: "break-all", fontSize: "0.95rem" }} aria-label="Impressão do certificado">{st.fingerprint}</code>
              <p className="meta" style={{ marginTop: 6 }}>
                Confira no celular se a impressão digital do certificado é a mesma antes de parear. O certificado é autoassinado, então o navegador vai avisar.
              </p>
            </div>
          )}
          <div className="row" style={{ marginBottom: 10 }}>
            <button className="btn" disabled={busy} onClick={makeQr}>{qr ? "Gerar novo QR" : "Gerar QR de pareamento"}</button>
            <button className="btn" disabled={busy} onClick={stop}>Desligar celular</button>
          </div>
          {qr && (
            <img
              alt="QR code de pareamento (válido por 10 minutos, uso único)"
              width={240}
              height={240}
              style={{ background: "#fff", padding: 8, borderRadius: 8, marginBottom: 12 }}
              src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(qr)}`}
            />
          )}
          <p className="meta" style={{ marginBottom: 12 }}>
            Um celular pareado pode ler a saída de todos os painéis deste computador. Pareie só aparelhos seus.
            Fora da sua rede local, use uma VPN ou um túnel que você controla (por exemplo, o Tailscale Serve apontando para a porta 8789). O Regente não tem servidor de retransmissão.
          </p>
        </>
      )}

      <h4>Aparelhos pareados</h4>
      {st.devices.length === 0 && <div className="meta">Nenhum aparelho pareado.</div>}
      {st.devices.length > 0 && (
        <table className="tbl" style={{ marginBottom: 8 }}>
          <tbody>
            {st.devices.map((d) => (
              <tr key={d.id}>
                <td>{d.name}</td>
                <td className="meta">{d.pending ? "aguardando aprovação" : d.lastSeen ? `visto ${new Date(d.lastSeen).toLocaleString("pt-BR")}` : "nunca usado"}</td>
                <td><button className="btn" onClick={() => revoke(d.id)}>Revogar</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {st.devices.length > 1 && (
        <button className="btn" onClick={() => revoke("all")}>Revogar todos</button>
      )}
    </>
  );
}
