import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { filterMemories, MEMORY_KINDS } from "../../lib/memory";
import { api, errMsg, on as ouvir, type EmbedStatus, type HygieneReport, type MemoryRow } from "../../lib/ipc";
import { useStore, wsOf } from "../../state/store";
import InstrucoesProjeto from "./InstrucoesProjeto";

type Aba = "memorias" | "sugestoes" | "arquivadas" | "globais" | "instrucoes";
const ABAS: [Aba, string][] = [["memorias", "Memórias"], ["sugestoes", "Sugestões"], ["arquivadas", "Arquivadas"], ["globais", "Preferências globais"], ["instrucoes", "Instruções do projeto"]];
/** Escopo das preferências globais no backend (`memory::GLOBAL`). */
const GLOBAL = "_global";
/** Dias de "esquecer" quando a opção é ligada. */
const DIAS_PADRAO = 90;

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const quando = (ms: number) => new Date(ms).toLocaleString("pt-BR", { dateStyle: "short", timeStyle: "short" });

/** Busca do FTS (com texto) ou a lista do workspace, por aba. Sugestões incluem as globais; preferências são só as globais. */
function carregar(aba: Aba, wsId: string, q: string): Promise<MemoryRow[]> {
  if (aba === "globais") return api.memoryList(GLOBAL, null);
  if (aba === "sugestoes") return Promise.all([api.memoryListStatus(wsId, "sugerida"), api.memoryListStatus(GLOBAL, "sugerida")]).then((l) => l.flat());
  if (aba === "arquivadas") return api.memoryListStatus(wsId, "arquivada");
  return q.trim() ? api.memorySearch(wsId, q.trim()) : api.memoryList(wsId, null);
}

const PROMPT_LIMPEZA = `Revise os arquivos de instrução deste projeto (CLAUDE.md, AGENTS.md e .claude/rules/) sem mudar o que eles mandam.
Mantenha o CLAUDE.md abaixo de 200 linhas: mova seções longas para arquivos em .claude/rules/ (com o campo paths) ou para skills.
Remova linhas repetidas entre arquivos e frases óbvias como "escreva código limpo".
Garanta uma seção "Compact instructions" dizendo o que a compactação deve preservar.
Mostre o diff antes de salvar.`;

/** Higiene das instruções (RM-22): custo fixo por turno e avisos. Só leitura; o prompt de limpeza é copiado, nunca aplicado. */
function HigienePanel({ wsId }: { wsId: string }) {
  const [rep, setRep] = useState<HygieneReport | null>(null);
  useEffect(() => {
    let alive = true;
    setRep(null);
    api.hygieneScan(wsId).then((r) => { if (alive) setRep(r); }).catch((e) => { if (alive) { setRep(null); toast(e); } });
    return () => { alive = false; };
  }, [wsId]);
  async function copiar() {
    try {
      await navigator.clipboard.writeText(PROMPT_LIMPEZA);
      useStore.getState().toast("Prompt de limpeza copiado.", "info");
    } catch (e) { toast(e); }
  }
  return (
    <div className="card">
      <strong>Higiene das instruções</strong>
      {rep === null ? <div className="meta" aria-live="polite">Lendo as instruções…</div> : (
        <>
          <p className="meta">Custo fixo estimado das instruções: ~{rep.tokensFixos} tokens por turno.</p>
          {rep.arquivos.length === 0 ? <div className="meta">Nenhum arquivo de instrução neste workspace.</div> : (
            <ul>{rep.arquivos.map((a) => <li key={a.path}>{a.path}: {a.linhas} linhas (~{a.tokensEst} tokens)</li>)}</ul>
          )}
          {rep.avisos.length > 0 && <ul>{rep.avisos.map((a, i) => <li key={i}>{a.arquivo}: {a.texto}</li>)}</ul>}
          <button className="btn" onClick={copiar}>Copiar prompt de limpeza</button>
        </>
      )}
    </div>
  );
}

export default function MemorySection() {
  const workspaces = useStore((s) => s.workspaces);
  const settings = useStore((s) => s.settings);
  const panes = useStore((s) => s.panes);
  const active = useStore((s) => s.active);
  const [wsId, setWsId] = useState<string | null>(active ?? workspaces[0]?.id ?? null);
  const [aba, setAba] = useState<Aba>("memorias");
  const [kind, setKind] = useState("todos");
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<MemoryRow[] | null>(null);
  const [editing, setEditing] = useState<{ id: number; text: string } | null>(null);
  const [novaGlobal, setNovaGlobal] = useState("");
  /** Texto do campo de dias enquanto o usuário digita (null = sem edição pendente). */
  const [diasTxt, setDiasTxt] = useState<string | null>(null);
  const ws = wsOf(wsId);
  const enabled = !!wsId && !(settings?.memoryDisabled ?? []).includes(wsId);
  const memoria = settings?.memoria ?? { aceitarSugestoes: false, esquecerDias: null };

  // a busca do FTS roda com o texto; a lista inteira filtra por tipo
  useEffect(() => {
    if (aba === "instrucoes") return; // a aba das instruções carrega o próprio conteúdo
    if (!wsId) { setRows([]); return; }
    let alive = true;
    setRows(null);
    const t = setTimeout(() => {
      carregar(aba, wsId, q).then((r) => { if (alive) setRows(r); }).catch((e) => { if (alive) { setRows([]); toast(e); } });
    }, 250);
    return () => { alive = false; clearTimeout(t); };
  }, [wsId, aba, q]);

  const recarregar = () => { if (wsId) carregar(aba, wsId, q).then(setRows, toast); };

  async function toggle(on: boolean) {
    if (!wsId || !settings) return;
    try {
      await api.memoryToggle(wsId, on);
      const disabled = settings.memoryDisabled.filter((w) => w !== wsId);
      useStore.getState().patch({ settings: { ...settings, memoryDisabled: on ? disabled : [...disabled, wsId] } });
    } catch (e) { toast(e); }
  }
  const salvarMemoria = (patch: Partial<typeof memoria>) => {
    if (!settings) return;
    useStore.getState().saveSettings({ ...settings, memoria: { ...memoria, ...patch } }).catch(toast);
  };
  // os dias são digitados: só salva 500 ms depois da última tecla
  useEffect(() => {
    if (diasTxt === null) return;
    const t = setTimeout(() => {
      salvarMemoria({ esquecerDias: Math.max(1, Math.min(3650, Number(diasTxt) || DIAS_PADRAO)) });
      setDiasTxt(null);
    }, 500);
    return () => clearTimeout(t);
  }, [diasTxt]);

  async function save() {
    if (!editing) return;
    try {
      await api.memoryUpdate(editing.id, editing.text);
      // na aba de sugestões, editar também aceita: o texto revisado vira memória ativa
      if (aba === "sugestoes") await api.memorySetStatus(editing.id, "ativa");
      setEditing(null);
      recarregar(); // a linha volta do backend (texto limpo e ordem atual)
    } catch (e) { toast(e); }
  }
  async function remove(r: MemoryRow) {
    if (!(await ask("Apagar esta memória? Não dá para desfazer.", { title: "Apagar memória", kind: "warning" }))) return;
    try {
      await api.memoryDelete(r.id);
      setRows((rs) => rs?.filter((x) => x.id !== r.id) ?? null);
    } catch (e) { toast(e); }
  }
  async function mudarEstado(r: MemoryRow, status: "ativa" | "sugerida" | "arquivada") {
    try {
      await api.memorySetStatus(r.id, status);
      setRows((rs) => rs?.filter((x) => x.id !== r.id) ?? null);
    } catch (e) { toast(e); }
  }
  async function fixar(r: MemoryRow) {
    try {
      await api.memorySetPinned(r.id, !r.pinned);
      setRows((rs) => rs?.map((x) => (x.id === r.id ? { ...x, pinned: !x.pinned } : x)) ?? null);
    } catch (e) { toast(e); }
  }
  async function adicionarGlobal() {
    const texto = novaGlobal.trim();
    if (!texto) return;
    try {
      await api.memoryAddGlobal(texto);
      setNovaGlobal("");
      recarregar();
    } catch (e) { toast(e); }
  }

  const shown = rows ? filterMemories(rows, aba === "memorias" ? kind : "todos", "") : null;
  const author = (r: MemoryRow) => (r.authorPane ? panes[r.authorPane]?.name ?? "painel fechado" : "—");

  /** Botões de uma linha, por aba. */
  function acoes(r: MemoryRow) {
    if (editing?.id === r.id) {
      return (
        <>
          <button className="btn primary" disabled={!editing.text.trim()} onClick={save}>{aba === "sugestoes" ? "Salvar e aceitar" : "Salvar"}</button>
          <button className="btn" onClick={() => setEditing(null)}>Cancelar</button>
        </>
      );
    }
    const editar = <button className="btn" onClick={() => setEditing({ id: r.id, text: r.text })}>Editar</button>;
    if (aba === "sugestoes") {
      return (
        <>
          <button className="btn primary" onClick={() => mudarEstado(r, "ativa")}>Aceitar</button>
          {editar}
          <button className="btn danger" onClick={() => api.memoryDelete(r.id).then(() => setRows((rs) => rs?.filter((x) => x.id !== r.id) ?? null), toast)}>Descartar</button>
        </>
      );
    }
    if (aba === "arquivadas") {
      return (
        <>
          <button className="btn" onClick={() => mudarEstado(r, "ativa")}>Reativar</button>
          <button className="btn danger" onClick={() => remove(r)}>Apagar</button>
        </>
      );
    }
    return (
      <>
        {aba === "memorias" && <button className="btn" aria-pressed={r.pinned} onClick={() => fixar(r)}>{r.pinned ? "Desafixar" : "Fixar"}</button>}
        {editar}
        <button className="btn danger" onClick={() => remove(r)}>Apagar</button>
      </>
    );
  }

  const vazio = q.trim() ? "Nenhuma memória encontrada." : aba === "sugestoes" ? "Nenhuma sugestão. Entregas, gates, integrações e arquivamentos geram sugestões aqui." : aba === "arquivadas" ? "Nenhuma memória arquivada." : "Nenhuma memória ainda.";

  return (
    <>
      <h3>Memória do workspace</h3>
      <p className="meta">O que os agentes gravaram sobre o projeto. Um painel novo recebe estas memórias ao abrir.</p>
      <div className="row" style={{ marginBottom: 8 }}>
        <label className="field"><span>Workspace</span>
          <select value={wsId ?? ""} onChange={(e) => setWsId(e.target.value || null)}>
            {workspaces.map((w) => <option key={w.id} value={w.id}>{w.name}</option>)}
          </select>
        </label>
        <label className="row"><input type="checkbox" checked={enabled} disabled={!wsId || !settings} onChange={(e) => toggle(e.target.checked)} /> Memória ligada</label>
      </div>
      {ws && !enabled && <div className="card" role="status">Memória desligada: os painéis novos deste workspace não recebem as memórias.</div>}
      {wsId && <HigienePanel wsId={wsId} />}
      <div role="tablist" aria-label="Partes da memória" className="row" style={{ marginBottom: 8, gap: 6 }}>
        {ABAS.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={aba === id} className={`btn ${aba === id ? "primary" : ""}`} onClick={() => { setAba(id); setEditing(null); }}>{label}</button>
        ))}
      </div>
      {aba === "instrucoes" ? <InstrucoesProjeto wsId={wsId} /> : (<>
      {aba !== "globais" && (
        <div className="row" style={{ marginBottom: 8 }}>
          <input placeholder="Buscar na memória" aria-label="Buscar na memória" value={q} onChange={(e) => setQ(e.target.value)} disabled={aba !== "memorias"} />
          <select aria-label="Tipo" value={kind} onChange={(e) => setKind(e.target.value)} disabled={aba !== "memorias"}>
            <option value="todos">Todos os tipos</option>
            {MEMORY_KINDS.map((k) => <option key={k} value={k}>{k}</option>)}
          </select>
        </div>
      )}
      {aba === "globais" && (
        <div className="card" style={{ marginBottom: 8 }}>
          <p className="meta">Preferências do usuário valem em todos os projetos (ex.: "use pnpm", "commits em pt-BR"). Os cinco primeiros entram no brief de cada painel, como dados, não instruções.</p>
          <div className="row">
            <input placeholder="Nova preferência" aria-label="Nova preferência global" maxLength={1000} value={novaGlobal} onChange={(e) => setNovaGlobal(e.target.value)} />
            <button className="btn primary" disabled={!novaGlobal.trim()} onClick={adicionarGlobal}>Adicionar</button>
          </div>
        </div>
      )}
      {rows === null ? <div className="meta" aria-live="polite">Carregando memórias…</div>
        : shown!.length === 0 ? <div className="meta">{vazio}</div>
        : (
          <table className="tbl">
            <thead><tr><th>Tipo</th><th>Texto</th><th>Autor</th><th>Data</th><th aria-label="Ações" /></tr></thead>
            <tbody>
              {shown!.map((r) => (
                <tr key={r.id}>
                  <td>{r.kind}{r.pinned && <span className="meta"> · fixada</span>}{r.status === "sugerida" && r.workspaceId === GLOBAL && <span className="meta"> · global</span>}</td>
                  <td>
                    {editing?.id === r.id ? (
                      <textarea aria-label="Editar memória" rows={3} maxLength={1000} value={editing.text} onChange={(e) => setEditing({ id: r.id, text: e.target.value })} />
                    ) : (
                      <span style={{ whiteSpace: "pre-wrap" }}>{r.text}</span>
                    )}
                  </td>
                  <td>{author(r)}</td>
                  <td>{quando(r.createdAt)}</td>
                  <td className="row">{acoes(r)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      <div className="card" style={{ marginTop: 12 }}>
        <h4>Captura e esquecimento</h4>
        <label className="row"><input type="checkbox" checked={memoria.aceitarSugestoes} onChange={(e) => salvarMemoria({ aceitarSugestoes: e.target.checked })} /> Aceitar sugestões automaticamente (desligado: você revisa na aba Sugestões)</label>
        <label className="row"><input type="checkbox" checked={memoria.esquecerDias !== null} onChange={(e) => salvarMemoria({ esquecerDias: e.target.checked ? DIAS_PADRAO : null })} /> Esquecer memórias não usadas após</label>
        {memoria.esquecerDias !== null && (
          <label className="field"><span>Dias (memórias fixadas nunca expiram; viram arquivadas, não são apagadas)</span>
            <input type="number" min={1} max={3650} value={diasTxt ?? memoria.esquecerDias} onChange={(e) => setDiasTxt(e.target.value)} />
          </label>
        )}
      </div>
      <BuscaSemantica />
      </>)}
    </>
  );
}

const megas = (bytes: number) => (bytes / 1_000_000).toFixed(0);

/** Busca por significado (opt-in). Ligar baixa o ONNX Runtime e o modelo uma vez, com progresso; sem isso, a busca é por palavra. */
function BuscaSemantica() {
  const settings = useStore((s) => s.settings);
  const [st, setSt] = useState<EmbedStatus | null>(null);
  const [baixando, setBaixando] = useState<{ nome: string; bytes: number; total: number } | null>(null);
  const [erro, setErro] = useState<string | null>(null);
  const [carregando, setCarregando] = useState(false);
  const ativo = !!settings?.buscaSemantica?.ativo;

  const recarregar = () => api.embedStatus().then(setSt, toast);
  useEffect(() => { recarregar(); }, []);
  // o carregamento do modelo roda no backend e avisa o resultado por evento
  useEffect(() => {
    const parar = ouvir<{ ativo: boolean; pronto: boolean; erro?: string | null }>("busca-semantica", (e) => {
      setCarregando(false);
      if (e.erro) setErro(e.erro);
      api.embedStatus().then(setSt, toast);
    });
    return () => { parar.then((f) => f()); };
  }, []);

  const gravarAtivo = (on: boolean) => {
    const s = useStore.getState().settings;
    if (s) useStore.getState().patch({ settings: { ...s, buscaSemantica: { ...s.buscaSemantica, ativo: on } } });
  };

  async function ligar(on: boolean) {
    setErro(null);
    try {
      if (on && !st?.instalado) {
        setBaixando({ nome: "ONNX Runtime", bytes: 0, total: 1 });
        await api.embedInstall((e) => { if (e.evento === "arquivo") setBaixando({ nome: e.nome, bytes: e.bytes, total: e.total }); });
        setBaixando(null);
      }
      if (on) setCarregando(true);
      await api.embedSetAtivo(on);
      gravarAtivo(on);
      recarregar();
    } catch (e) {
      setBaixando(null);
      setCarregando(false);
      setErro(errMsg(e));
      recarregar();
    }
  }

  async function remover() {
    setErro(null);
    try {
      await api.embedRemove();
      gravarAtivo(false);
      recarregar();
    } catch (e) { setErro(errMsg(e)); }
  }

  return (
    <div className="card" style={{ marginTop: 12 }}>
      <h4>Busca por significado (local)</h4>
      <p className="muted">
        Acha memórias pelo sentido, não só pela palavra. Roda no seu computador e precisa baixar uma vez o ONNX Runtime ({st?.onnxruntimeMb ?? "…"} MB) e o modelo multilíngue e5-small ({st?.modeloMb ?? "…"} MB). Sem isso, a busca continua por palavra.
      </p>
      {st && !st.instalado && !baixando && <button onClick={() => ligar(true)}>Baixar e ligar (~{st.tamanhoMb} MB)</button>}
      {baixando && (
        <p>
          Baixando {baixando.nome}: {megas(baixando.bytes)} de {megas(baixando.total)} MB
          <progress value={baixando.bytes} max={baixando.total} style={{ display: "block", width: "100%" }} />
        </p>
      )}
      {baixando && <button onClick={() => api.embedCancel().catch(toast)}>Cancelar</button>}
      {carregando && <p className="muted">Carregando o modelo…</p>}
      {st?.instalado && (
        <label className="row"><input type="checkbox" checked={ativo} disabled={carregando} onChange={(e) => ligar(e.target.checked)} /> Buscar por significado</label>
      )}
      {st?.instalado && !ativo && <button onClick={remover}>Remover arquivos baixados</button>}
      {erro && <p className="erro">{erro}</p>}
    </div>
  );
}
