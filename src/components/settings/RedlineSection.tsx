import { useEffect, useState } from "react";
import { api, errMsg, type RedlinePrefs } from "../../lib/ipc";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const DEFAULT = { ativa: true, noTopo: false };

/** Settings → Redline: por conta, `ativa` (ler o limite) e `no topo` (aparece no chip da barra superior). */
export default function RedlineSection() {
  const [prefs, setPrefs] = useState<RedlinePrefs | null>(null);
  const [accounts, setAccounts] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .redlinePrefs()
      .then((r) => {
        setAccounts(r.accounts);
        setPrefs({ accounts: Object.fromEntries(r.accounts.map((k) => [k, { ...DEFAULT, ...r.prefs.accounts[k] }])) });
      })
      .catch(toast);
  }, []);

  if (!prefs) return <><h3>Limites de uso</h3><div className="meta">Carregando…</div></>;

  const set = (k: string, p: Partial<(typeof DEFAULT)>) => setPrefs({ accounts: { ...prefs.accounts, [k]: { ...prefs.accounts[k], ...p } } });

  async function save() {
    if (!prefs || busy) return;
    setBusy(true);
    try {
      await api.redlineSetPrefs(prefs);
      useStore.getState().toast("Limites de uso salvos", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h3>Limites de uso</h3>
      <div className="meta" style={{ marginBottom: 8 }}>Limites lidos a cada 5 min, só leitura. Cada conta pode ser desligada ou marcada para o cabeçalho. Sem nenhuma marcada, as contas do Claude aparecem no cabeçalho (5h e 7d).</div>
      {accounts.length === 0 && <div className="meta">Nenhuma conta conhecida.</div>}
      <table className="tbl">
        <thead><tr><th>Conta</th><th>Ativa</th><th>No topo</th></tr></thead>
        <tbody>
          {accounts.map((k) => (
            <tr key={k}>
              <td>{k.startsWith("claude") ? "Claude" : "Codex"} · {k.split("/")[1]}</td>
              <td><input type="checkbox" aria-label={`Ativa: ${k}`} checked={prefs.accounts[k].ativa} onChange={(e) => set(k, { ativa: e.target.checked })} /></td>
              <td><input type="checkbox" aria-label={`No topo: ${k}`} checked={prefs.accounts[k].noTopo} onChange={(e) => set(k, { noTopo: e.target.checked })} /></td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row" style={{ marginTop: 10 }}>
        <button className="btn" disabled={busy} onClick={save}>Salvar</button>
      </div>
    </>
  );
}
