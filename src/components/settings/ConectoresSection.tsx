import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errMsg, type Conector, type ConectorImportavel } from "../../lib/ipc";

const EXEMPLO = `{
  "type": "http",
  "url": "https://exemplo.com/mcp",
  "headers": { "Authorization": "Bearer SUA_CHAVE" }
}`;

/** Conectores MCP: cadastrados uma vez, entregues a todo painel do Claude, seja qual for a conta. */
export default function ConectoresSection() {
  const [lista, setLista] = useState<Conector[]>([]);
  const [importaveis, setImportaveis] = useState<ConectorImportavel[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [chave7, setChave7] = useState("");
  const [nome, setNome] = useState("");
  const [config, setConfig] = useState("");
  const loadImportaveis = () => api.conectoresImportaveis().then(setImportaveis).catch(() => setImportaveis([]));
  useEffect(() => {
    api.conectoresList().then((l) => { setLista(l); setErr(null); }).catch((e) => setErr(errMsg(e)));
    loadImportaveis();
  }, []);
  /** Toda ação devolve a lista nova; os importáveis mudam junto. */
  const act = (p: Promise<Conector[]>, depois?: () => void) =>
    p.then((l) => { setLista(l); setErr(null); depois?.(); loadImportaveis(); }).catch((e) => setErr(errMsg(e)));

  async function remove(n: string) {
    if (!(await ask(`Remover o conector "${n}"? A configuração sai do Cofre. Os painéis já abertos mantêm o que receberam.`, { title: "Remover conector", kind: "warning" }))) return;
    act(api.conectorDelete(n));
  }
  const temContext7 = lista.some((c) => c.nome === "context7");

  return (
    <>
      <h3>Conectores MCP</h3>
      <p style={{ color: "var(--fg3)" }}>
        Cadastre um servidor MCP uma vez e todo painel do Claude o recebe, em qualquer conta, sem configurar conta por conta. A
        configuração fica cifrada no Cofre e vale para os painéis que abrirem ou reiniciarem depois da mudança. O perfil MCP mínimo e os
        workspaces em modo sensível não recebem conectores. Conectores com login do claude.ai (Gmail, ClickUp…) continuam por conta.
      </p>
      {err && <div className="card" role="alert">{err}</div>}
      <table className="tbl">
        <tbody>
          {lista.map((c) => (
            <tr key={c.nome}>
              <td>
                <label className="row">
                  <input type="checkbox" checked={c.ativo} aria-label={`Ligar ${c.nome}`} onChange={(e) => act(api.conectorToggle(c.nome, e.target.checked))} />
                  {c.nome}
                </label>
              </td>
              <td>{c.transporte}</td>
              <td style={{ color: "var(--fg3)", wordBreak: "break-all" }}>{c.resumo}</td>
              <td><button className="btn danger" onClick={() => remove(c.nome)}>Remover</button></td>
            </tr>
          ))}
          {lista.length === 0 && <tr><td className="empty" style={{ padding: 8 }}>Nenhum conector.</td></tr>}
        </tbody>
      </table>

      <h3 style={{ marginTop: 16 }}>Context7</h3>
      <p style={{ color: "var(--fg3)" }}>Documentação atualizada de bibliotecas. Funciona sem chave; com a chave (context7.com/dashboard), o limite de consultas é maior.</p>
      <div className="row">
        <input type="password" placeholder="chave (opcional)" aria-label="Chave do Context7" value={chave7} onChange={(e) => setChave7(e.target.value)} />
        <button className="btn primary" onClick={() => act(api.conectorContext7(chave7.trim() || null), () => setChave7(""))}>
          {temContext7 ? "Salvar chave" : "Adicionar Context7"}
        </button>
      </div>

      {importaveis.length > 0 && (
        <>
          <h3 style={{ marginTop: 16 }}>Trazer das suas contas</h3>
          <p style={{ color: "var(--fg3)" }}>MCPs já configurados em uma conta do Claude. Importar copia a configuração para cá (a conta não muda) e passa a valer em todas.</p>
          <table className="tbl">
            <tbody>
              {importaveis.map((i) => (
                <tr key={`${i.conta}/${i.nome}`}>
                  <td>{i.nome}</td>
                  <td>{i.transporte}</td>
                  <td style={{ color: "var(--fg3)" }}>conta {i.conta}</td>
                  <td><button className="btn" onClick={() => act(api.conectorImport(i.nome, i.conta))}>Importar</button></td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}

      <h3 style={{ marginTop: 16 }}>Outro conector</h3>
      <p style={{ color: "var(--fg3)" }}>
        No formato do <code>mcpServers</code> do Claude: <code>url</code> (http ou sse) ou <code>command</code> com <code>args</code> e <code>env</code>.
      </p>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        <input placeholder="nome (ex.: github)" aria-label="Nome do conector" value={nome} onChange={(e) => setNome(e.target.value)} />
        <textarea rows={6} placeholder={EXEMPLO} aria-label="Configuração do conector (JSON)" value={config} onChange={(e) => setConfig(e.target.value)} style={{ fontFamily: "monospace" }} />
        <div className="row">
          <button className="btn primary" disabled={!nome.trim() || !config.trim()} onClick={() => act(api.conectorSave(nome.trim(), config), () => { setNome(""); setConfig(""); })}>
            Salvar conector
          </button>
        </div>
      </div>
    </>
  );
}
