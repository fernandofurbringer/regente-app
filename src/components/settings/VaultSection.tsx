import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errMsg } from "../../lib/ipc";
import { parseVaultName, scopedVaultName } from "../../lib/vault";
import { useStore } from "../../state/store";

export default function VaultSection() {
  const [names, setNames] = useState<string[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  /** escopo do segredo novo: "" = global; senão o id do workspace */
  const [scope, setScope] = useState("");
  const workspaces = useStore((s) => s.workspaces);
  const settings = useStore((s) => s.settings);
  const sensitive = settings?.sensitiveWorkspaces ?? [];
  const wsName = (id: string) => workspaces.find((w) => w.id === id)?.name ?? "workspace removido";
  const load = () => api.vaultNames().then((n) => { setNames(n); setErr(null); }).catch((e) => setErr(errMsg(e)));
  useEffect(() => { load(); }, []);
  async function remove(n: string) {
    if (!(await ask(`Excluir o segredo "${n}"? Não dá para desfazer.`, { title: "Excluir segredo", kind: "warning" }))) return;
    api.vaultDelete(n).then(load).catch((e) => setErr(errMsg(e)));
  }
  async function toggleSensitive(wsId: string, on: boolean) {
    try {
      await useStore.getState().setWorkspaceSensitive(wsId, on);
      // painéis já abertos mantêm o que receberam; o modo vale para os próximos
      useStore.getState().toast(on ? "Modo sensível ligado. Painéis já abertos só perdem o segredo ao reiniciar." : "Modo sensível desligado.", "info");
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }
  if (err) {
    return (
      <>
        <h3>Cofre</h3>
        <div className="card" role="alert">{err}<div className="actions"><button className="btn" onClick={load}>Tentar de novo</button></div></div>
      </>
    );
  }
  return (
    <>
      <h3>Cofre</h3>
      <p style={{ color: "var(--fg3)" }}>Segredos cifrados (AES-256-GCM), com a chave-mestra no Gerenciador de Credenciais do Windows. Depois de salvos, os valores não voltam para a tela.</p>
      <table className="tbl">
        <tbody>
          {names.map((n) => {
            const p = parseVaultName(n);
            return (
              <tr key={n}>
                <td>{p.name}</td>
                <td>{p.scope ? `workspace: ${wsName(p.scope)}` : "global"}</td>
                <td>••••••</td>
                <td><button className="btn danger" onClick={() => remove(n)}>Excluir</button></td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="row" style={{ marginTop: 10 }}>
        <input placeholder="nome (ex.: route.openrouter)" aria-label="Nome do segredo" value={name} onChange={(e) => setName(e.target.value)} />
        <select aria-label="Escopo do segredo" value={scope} onChange={(e) => setScope(e.target.value)}>
          <option value="">global (todos os workspaces)</option>
          {workspaces.map((w) => <option key={w.id} value={w.id}>só {w.name}</option>)}
        </select>
        <input type="password" placeholder="valor" aria-label="Valor do segredo" value={value} onChange={(e) => setValue(e.target.value)} />
        <button className="btn primary" disabled={!name.trim() || !value}
          onClick={() => api.vaultSet(scopedVaultName(scope || null, name), value).then(() => { setName(""); setValue(""); return load(); }).catch((e) => setErr(errMsg(e)))}>Salvar</button>
      </div>
      <h3 style={{ marginTop: 16 }}>Modo sensível por workspace</h3>
      <p style={{ color: "var(--fg3)" }}>Com o modo ligado, rotas que usam o Cofre não abrem painéis neste workspace, e a tela de detalhes mascara os valores do Cofre.</p>
      {workspaces.map((w) => (
        <label key={w.id} className="row">
          <input type="checkbox" checked={sensitive.includes(w.id)} onChange={(e) => toggleSensitive(w.id, e.target.checked)} />
          <span>{w.name}</span>
        </label>
      ))}
      {workspaces.length === 0 && <div className="empty" style={{ padding: 8 }}>Abra um workspace para configurar.</div>}
    </>
  );
}
