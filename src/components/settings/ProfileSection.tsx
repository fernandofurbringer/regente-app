import { useEffect, useState } from "react";
import { api, errMsg, on, type Ajuste, type AutoAjuste, type AutoAjusteModo, type Perfil, type PerfisInfo, type PerfisPrefs } from "../../lib/ipc";
import { useStore } from "../../state/store";
import PapeisTable from "../PapeisTable";

const SLOTS: [keyof Perfil["modelos"], string][] = [
  ["planejar", "Planejar"], ["implementar", "Implementar"], ["revisar", "Revisar"], ["testar", "Testar"], ["explorar", "Explorar"],
];
/** Nome em português de cada tipo de tarefa (o mesmo que a tela de perfil mostra). */
export const TIPO_TAREFA_LABEL: Record<string, string> = Object.fromEntries(SLOTS);
const AJUDA: Record<string, string> = {
  economico: "Econômico: menos tokens, recomeça em vez de compactar.",
  equilibrado: "Equilibrado: o padrão recomendado.",
  precisao: "Máxima precisão: modelos maiores e mais contexto.",
};
const QUE: Record<string, string> = {
  compactacao: "Compactação", recomeco: "Recomeço com memória", escalonamento: "Escalonamento de modelo", perfil_patch: "Ajuste de perfil",
  indice_codigo: "Índice de código", testes_relacionados: "Testes relacionados", limites_saida: "Limites de saída",
};

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const MODOS: [AutoAjusteModo, string][] = [["aplicar", "Aplicar sozinho"], ["propor", "Só sugerir"], ["desligado", "Desligado"]];
/** Modelo curto para a tabela: `claude-opus-5-5` vira `opus-5-5`. */
const curto = (m: string) => m.replace(/^claude-/, "");

export default function ProfileSection() {
  const workspaces = useStore((s) => s.workspaces);
  const [info, setInfo] = useState<PerfisInfo | null>(null);
  const [tab, setTab] = useState<"perfis" | "ajustes">("perfis");
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [ajustes, setAjustes] = useState<Ajuste[] | null>(null);
  const [denies, setDenies] = useState<string[] | null>(null);
  const [auto, setAuto] = useState<AutoAjuste | null>(null);
  /** "Quem faz o quê": qual perfil a tabela edita e ele já com a edição do usuário (preset + editados) */
  const [papelId, setPapelId] = useState<string | null>(null);
  const [papelPerfil, setPapelPerfil] = useState<Perfil | null>(null);
  useEffect(() => {
    const id = papelId ?? info?.padrao;
    if (!id) return;
    // workspace vazio: sem o ajuste de projeto, só o preset e a edição geral
    api.perfilPreview("", id).then(setPapelPerfil).catch(toast);
  }, [papelId, info]);
  useEffect(() => { api.perfilReadDenies().then(setDenies).catch(() => setDenies([])); }, []);

  const load = () => api.perfisGet().then(setInfo).catch(toast);
  const refreshSettings = () => api.settingsGet().then((s) => useStore.getState().patch({ settings: s })).catch(toast);
  const loadAjustes = () => api.ajustesList(null).then(setAjustes).catch((e) => { setAjustes([]); toast(e); });
  useEffect(() => { load(); }, []);
  useEffect(() => {
    if (tab !== "ajustes") return;
    loadAjustes();
    api.autoajusteGet().then(setAuto).catch(toast);
    const uns = ["ajuste-novo", "ajuste-atualizado"].map((ev) => on(ev, () => { loadAjustes(); }));
    return () => { uns.forEach((u) => { void u.then((f) => f()); }); };
  }, [tab]);

  if (!info) return <><h3>Perfil de trabalho</h3><div className="meta">Carregando perfis…</div></>;

  const prefs = (over: Partial<PerfisPrefs>): PerfisPrefs => ({ padrao: info.padrao, editados: info.editados, patches: info.patches, ...over });
  async function save(next: PerfisPrefs): Promise<boolean> {
    try {
      await api.perfisSave(next);
      setErr(null);
      await load();
      await refreshSettings();
      return true;
    } catch (e) {
      setErr(errMsg(e));
      return false;
    }
  }
  function startEdit(p: Perfil) {
    setEditing(p.id);
    setDraft(JSON.stringify(info!.editados[p.id] ?? p, null, 2));
    setErr(null);
  }
  async function saveEdit(id: string) {
    let parsed: Perfil;
    try {
      parsed = JSON.parse(draft);
    } catch (e) {
      setErr(`JSON inválido: ${errMsg(e)}`);
      return;
    }
    if (await save(prefs({ editados: { ...info!.editados, [id]: parsed } }))) setEditing(null);
  }
  async function restore(id: string) {
    try {
      await api.perfilRestore(id);
      setEditing(null);
      await load();
      await refreshSettings();
    } catch (e) { toast(e); }
  }
  async function removePatch(ws: string) {
    try {
      await api.perfilPatchRemove(ws);
      setErr(null);
      await load();
      loadAjustes();
    } catch (e) { toast(e); }
  }
  async function undo(a: Ajuste) {
    try {
      await api.ajustesUndo(a.id);
      useStore.getState().toast("Ajuste desfeito.", "ok");
      await loadAjustes();
      await load();
      await refreshSettings();
    } catch (e) { toast(e); }
  }
  async function ignorar(a: Ajuste) {
    try {
      await api.ajustesIgnorar(a.id);
      useStore.getState().toast("Sugestão ignorada.", "ok");
      await loadAjustes();
    } catch (e) { toast(e); }
  }
  async function zerarAprendizado() {
    try {
      await api.autoajusteZerarAprendizado();
      setAuto((a) => (a ? { ...a, aprendido: { k: null, rResumo: null, fatorRestante: null } } : a));
    } catch (e) { toast(e); }
  }
  async function setModo(modo: AutoAjusteModo) {
    try {
      await api.autoajusteSetModo(modo);
      setAuto((a) => (a ? { ...a, modo } : a));
    } catch (e) { toast(e); }
  }
  async function aplicarProposta(a: Ajuste) {
    try {
      await api.ajustesAplicar(a.id);
      useStore.getState().toast("Sugestão aplicada.", "ok");
      await loadAjustes();
      await load();
    } catch (e) { toast(e); }
  }
  const nomeWs = (id: string) => workspaces.find((w) => w.id === id)?.name ?? id;
  // proposta superada: outra linha mais nova do mesmo workspace (o patch pode ter mudado desde então)
  const superada = (a: Ajuste) => (ajustes ?? []).some((b) => b.workspaceId === a.workspaceId && b.createdAt > a.createdAt);
  const decimal = (x: number) => x.toFixed(1).replace(".", ",");

  return (
    <>
      <h3>Perfil de trabalho</h3>
      <div className="card" style={{ marginBottom: 8 }}>
        <strong>Pastas que a IA não deve ler</strong>
        <ul className="meta">{(denies ?? []).map((r) => <li key={r}><code>{r}</code></li>)}</ul>
        <p className="meta">O arquivo .claudeignore não funciona no Claude Code; o Regente usa regras de permissão.</p>
      </div>
      <div className="row" role="tablist" aria-label="Seções do perfil" style={{ gap: 6, marginBottom: 8 }}>
        <button className={`btn ${tab === "perfis" ? "primary" : ""}`} role="tab" aria-selected={tab === "perfis"} onClick={() => setTab("perfis")}>Perfis</button>
        <button className={`btn ${tab === "ajustes" ? "primary" : ""}`} role="tab" aria-selected={tab === "ajustes"} onClick={() => setTab("ajustes")}>Ajustes automáticos</button>
      </div>

      {tab === "perfis" && (
        <>
          <label className="row" style={{ gap: 8 }}>
            Perfil padrão
            <select value={info.padrao} onChange={(e) => { void save(prefs({ padrao: e.target.value })); }}>
              {info.presets.map((p) => <option key={p.id} value={p.id}>{p.nome}</option>)}
            </select>
          </label>
          <ul className="meta" style={{ margin: "6px 0 12px" }}>
            {Object.values(AJUDA).map((t) => <li key={t}>{t}</li>)}
          </ul>
          <p className="meta">Vale só para painéis novos. Painéis abertos mantêm o perfil com que nasceram.</p>

          <table className="tbl">
            <thead>
              <tr><th>Perfil</th>{SLOTS.map(([, nome]) => <th key={nome}>{nome}</th>)}<th>Janela</th><th>K</th><th>Bash máx.</th><th aria-label="Ações" /></tr>
            </thead>
            <tbody>
              {info.presets.map((p) => (
                <tr key={p.id}>
                  <td>{p.nome}{info.editados[p.id] && <div className="meta">editado</div>}</td>
                  {SLOTS.map(([slot]) => <td key={slot}>{curto(p.modelos[slot].model)} · {p.modelos[slot].effort}</td>)}
                  <td>{p.contexto.janelaLongos.toLocaleString("pt-BR")}</td>
                  <td>{p.contexto.paybackK}</td>
                  <td>{p.saidas.bashMax ?? "padrão do CLI"}</td>
                  <td>
                    <button className="btn" onClick={() => startEdit(p)}>Editar valores (avançado)</button>
                    {info.editados[p.id] && <button className="btn" onClick={() => { void restore(p.id); }}>Restaurar padrão</button>}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {editing && (
            <div style={{ marginTop: 12 }}>
              <label>
                Valores do perfil {info.presets.find((p) => p.id === editing)?.nome} (JSON completo)
                <textarea rows={16} value={draft} onChange={(e) => setDraft(e.target.value)} style={{ width: "100%", fontFamily: "monospace" }} />
              </label>
              {err && <div className="meta" role="alert" style={{ color: "var(--erro, #e5484d)" }}>{err}</div>}
              <div className="row" style={{ gap: 6, marginTop: 6 }}>
                <button className="btn primary" onClick={() => { void saveEdit(editing); }}>Salvar</button>
                <button className="btn" onClick={() => { setEditing(null); setErr(null); }}>Cancelar</button>
              </div>
            </div>
          )}
          {!editing && err && <div className="meta" role="alert" style={{ color: "var(--erro, #e5484d)" }}>{err}</div>}

          <h4 style={{ marginTop: 16 }}>Quem faz o quê</h4>
          <p className="meta">
            A IA, o modelo e o esforço de cada papel da missão agêntica e de cada tipo de executor. Vale mais que o arquivo
            do agente e que o pedido do orquestrador: nada roda em Opus se você não escolher. Misture IAs à vontade
            (por exemplo, olheiros no Gemini e executores no Codex).
          </p>
          <label className="row" style={{ gap: 8, marginBottom: 6 }}>
            Perfil
            <select value={papelId ?? info.padrao} onChange={(e) => setPapelId(e.target.value)}>
              {info.presets.map((p) => <option key={p.id} value={p.id}>{p.nome}</option>)}
            </select>
          </label>
          {papelPerfil && (
            <PapeisTable perfil={papelPerfil} onSave={async (campos) => {
              const id = papelId ?? info.padrao;
              if (await save(prefs({ editados: { ...info.editados, [id]: { ...papelPerfil, ...campos } } }))) {
                useStore.getState().toast("Papéis salvos: valem para as próximas missões.", "ok");
              }
            }} />
          )}

          <h4 style={{ marginTop: 16 }}>Ajustes por workspace</h4>
          {Object.keys(info.patches).length === 0
            ? <div className="meta">Nenhum workspace com ajuste próprio.</div>
            : Object.entries(info.patches).map(([ws, patch]) => (
              <div key={ws} className="row" style={{ justifyContent: "space-between", gap: 8 }}>
                <span>{nomeWs(ws)} <span className="meta">{JSON.stringify(patch)}</span></span>
                <button className="btn" onClick={() => { void removePatch(ws); }}>Remover ajuste</button>
              </div>
            ))}
        </>
      )}

      {tab === "ajustes" && (
        <>
          <div className="row" role="group" aria-label="Piloto automático de eficiência" style={{ gap: 6, marginBottom: 8, alignItems: "center", flexWrap: "wrap" }}>
            <span>Piloto automático de eficiência:</span>
            {MODOS.map(([id, nome]) => (
              <button key={id} className={`btn ${auto?.modo === id ? "primary" : ""}`} aria-pressed={auto?.modo === id} onClick={() => { void setModo(id); }}>{nome}</button>
            ))}
          </div>
          {auto && (auto.aprendido.k !== null || auto.aprendido.fatorRestante !== null) && (
            <div className="row" style={{ gap: 8, marginBottom: 8, alignItems: "center" }}>
              <span className="meta">
                Aprendido: K do perfil {auto.aprendido.k !== null ? `${auto.aprendido.k >= 0 ? "+" : ""}${decimal(auto.aprendido.k)}` : "sem ajuste"}
                {auto.aprendido.fatorRestante !== null ? `, fator de turnos restantes ${decimal(auto.aprendido.fatorRestante)}` : ""}.
              </span>
              <button className="btn" onClick={() => { void zerarAprendizado(); }}>Zerar aprendizado</button>
            </div>
          )}
          {ajustes === null ? <div className="meta">Carregando ajustes…</div>
          : ajustes.length === 0 ? <div className="meta">Nenhum ajuste automático ainda.</div>
          : (
            <table className="tbl">
              <thead><tr><th>Quando</th><th>O quê</th><th>Por quê</th><th>Efeito</th><th aria-label="Ações" /></tr></thead>
              <tbody>
                {ajustes.map((a) => (
                  <tr key={a.id}>
                    <td>{new Date(a.createdAt).toLocaleString("pt-BR")}</td>
                    <td>{QUE[a.kind] ?? a.kind} <span className="meta">· {a.acao}</span></td>
                    <td>{a.motivo}</td>
                    <td className="meta">{a.efeitoJson ? `medido: ${a.efeitoJson}` : a.estimativaJson ? `estimado: ${a.estimativaJson}` : "—"}</td>
                    <td>
                      {a.desfeitoAt ? <span className="meta">desfeito</span>
                        : a.desfazivel ? <button className="btn" onClick={() => { void undo(a); }}>Desfazer</button>
                        : a.acao === "proposto" && a.depoisJson && !superada(a) ? <>
                          <button className="btn primary" onClick={() => { void aplicarProposta(a); }}>Aplicar</button>
                          <button className="btn" onClick={() => { void ignorar(a); }}>Ignorar</button>
                        </>
                        : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
    </>
  );
}
