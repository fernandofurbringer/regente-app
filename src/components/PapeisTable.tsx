import { useEffect, useState } from "react";
import { api, errMsg, type Perfil, type PerfilSlot } from "../lib/ipc";
import { camposDe, LINHAS, limiteDoPapel, tabelaDe, type Tabela } from "../lib/papeis";
import { colorFor, labelOf } from "../lib/redline";
import { useStore } from "../state/store";

const ESFORCOS = ["low", "medium", "high", "xhigh", "max"];

/**
 * "Quem faz o quê": CLI, modelo e esforço de cada papel da missão agêntica e de cada tipo de executor. É a escolha do
 * usuário: vence o arquivo do agente e o pedido do orquestrador. Ao lado, o limite que cada papel consome (Redline).
 * `onSave` recebe os campos `papeis` e `modelos` do perfil.
 */
export default function PapeisTable({ perfil, onSave, salvarRotulo = "Salvar", soPapeis = false }: {
  perfil: Perfil;
  onSave(campos: Pick<Perfil, "papeis" | "modelos">): Promise<void>;
  salvarRotulo?: string;
  /** só os papéis da missão (o assistente esconde os executores para caber) */
  soPapeis?: boolean;
}) {
  const providers = useStore((s) => s.providers);
  const limits = useStore((s) => s.limits);
  const [t, setT] = useState<Tabela>(() => tabelaDe(perfil));
  const [busy, setBusy] = useState(false);
  useEffect(() => setT(tabelaDe(perfil)), [perfil]);
  const original = JSON.stringify(tabelaDe(perfil));
  const mudou = JSON.stringify(t) !== original;
  const clis = providers?.clis ?? [];
  const set = (chave: string, v: PerfilSlot | null) => setT((x) => ({ ...x, [chave]: v }));
  const linhas = soPapeis ? LINHAS.filter((l) => l.grupo === "papeis") : LINHAS;

  async function salvar() {
    setBusy(true);
    try {
      await onSave(camposDe(t, perfil));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="papeis">
      <table className="tbl">
        <thead><tr><th>Papel</th><th>IA</th><th>Modelo</th><th>Esforço</th><th>Limite</th></tr></thead>
        <tbody>
          {linhas.map((l) => {
            const v = t[l.chave];
            const cli = clis.find((c) => c.id === (v?.cli ?? "claude"));
            const lim = v ? limiteDoPapel(limits, v) : null;
            const listId = `papel-modelos-${l.chave}`;
            return (
              <tr key={l.chave}>
                <td title={l.dica}>{l.nome}<div className="meta">{l.dica}</div></td>
                {l.chave === "cruzado" && !v ? (
                  <td colSpan={4}>
                    <button type="button" className="btn" onClick={() => set("cruzado", { cli: "codex", model: "", effort: "medium" })}>Ligar revisão cruzada</button>
                  </td>
                ) : (
                  <>
                    <td>
                      <select aria-label={`IA de ${l.nome}`} value={v?.cli ?? "claude"} onChange={(e) => set(l.chave, { cli: e.target.value, model: "", effort: v?.effort ?? "medium" })}>
                        {clis.map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
                      </select>
                    </td>
                    <td>
                      <input aria-label={`Modelo de ${l.nome}`} list={listId} value={v?.model ?? ""} placeholder="modelo" style={{ width: "100%" }}
                        onChange={(e) => set(l.chave, { cli: v?.cli ?? "claude", model: e.target.value.trim(), effort: v?.effort ?? "medium" })} />
                      <datalist id={listId}>{(cli?.models ?? []).map((m) => <option key={m} value={m} />)}</datalist>
                    </td>
                    <td>
                      <select aria-label={`Esforço de ${l.nome}`} value={v?.effort ?? "medium"} onChange={(e) => set(l.chave, { cli: v?.cli ?? "claude", model: v?.model ?? "", effort: e.target.value })}>
                        {(cli?.efforts.length ? cli.efforts : ESFORCOS).map((x) => <option key={x}>{x}</option>)}
                      </select>
                    </td>
                    <td>
                      {lim ? <span className="chip" style={{ color: colorFor(lim.pct) }} title={`Janela ${lim.janela} da conta principal`}>{labelOf(lim.janela)} {Math.round(lim.pct)}%</span> : <span className="meta">—</span>}
                      {l.chave === "cruzado" && <button type="button" className="btn" style={{ marginLeft: 6 }} onClick={() => set("cruzado", null)}>Desligar</button>}
                    </td>
                  </>
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="row" style={{ gap: 6, marginTop: 6 }}>
        <button type="button" className="btn primary" disabled={!mudou || busy || linhas.some((l) => t[l.chave] && !t[l.chave]!.model)} onClick={() => void salvar()}>{busy ? "Salvando…" : salvarRotulo}</button>
        {mudou && <button type="button" className="btn" onClick={() => setT(JSON.parse(original))}>Desfazer</button>}
        {linhas.some((l) => t[l.chave] && !t[l.chave]!.model) && <span className="meta">Escolha o modelo de cada papel.</span>}
      </div>
    </div>
  );
}

/**
 * No assistente da missão agêntica: o resumo de quem faz o quê neste projeto e, em "Ajustar", a tabela. A edição vira o
 * ajuste do projeto (vale para as missões deste workspace, em qualquer perfil).
 */
export function PapeisDoProjeto({ wsId, profile }: { wsId: string; profile: string | null }) {
  const [p, setP] = useState<Perfil | null>(null);
  const [aberto, setAberto] = useState(false);
  useEffect(() => {
    let vivo = true;
    api.perfilPreview(wsId, profile).then((x) => { if (vivo) setP(x); }).catch(() => { if (vivo) setP(null); });
    return () => { vivo = false; };
  }, [wsId, profile]);
  if (!p) return null;
  const curto = (s: PerfilSlot | null) => (s ? `${s.cli && s.cli !== "claude" ? `${s.cli} ` : ""}${s.model.replace(/^claude-/, "")}` : "desligado");
  async function salvar(campos: Pick<Perfil, "papeis" | "modelos">) {
    try {
      const info = await api.perfisGet();
      const atual = (info.patches[wsId] ?? {}) as Record<string, unknown>;
      await api.perfisSave({ padrao: info.padrao, editados: info.editados, patches: { ...info.patches, [wsId]: { ...atual, ...campos } } });
      setP(await api.perfilPreview(wsId, profile));
      useStore.getState().toast("Papéis deste projeto salvos.", "ok");
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }
  return (
    <div className="field">
      <span>Quem faz o quê</span>
      <div className="meta">
        piloto {curto(p.papeis.piloto)} · scout {curto(p.papeis.scout)} · builder {curto(p.papeis.builder)} · reviewer {curto(p.papeis.reviewer)} · executores {curto(p.modelos.implementar)} · revisão cruzada {curto(p.papeis.cruzado)}
      </div>
      <div className="row" style={{ gap: 6 }}>
        <button type="button" className="btn" aria-expanded={aberto} onClick={() => setAberto((x) => !x)}>{aberto ? "Fechar" : "Ajustar papéis"}</button>
        <span className="meta">O campo Piloto acima, se preenchido, vence a linha do piloto.</span>
      </div>
      {aberto && <PapeisTable perfil={p} onSave={salvar} salvarRotulo="Salvar para este projeto" />}
    </div>
  );
}
