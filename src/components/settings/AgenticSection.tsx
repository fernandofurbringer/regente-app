import { useEffect, useState } from "react";
import { baseUrlError, linesToList, listToLines, short, slugId, spend, spendByType, typeName } from "../../lib/agentic";
import { fmtUsd } from "../../lib/format";
import { api, errMsg, type AgenticConfig, type AgentDef, type Decision, type Perfil, type SkillDef, type TaskType } from "../../lib/ipc";
import { useStore } from "../../state/store";
import Modal from "../Modal";

const DAY = 86_400_000;
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const KIND: Record<string, string> = { none: "Nenhum — o piloto escolhe", openai: "Compatível com OpenAI (ex.: OpenRouter)", anthropic: "Anthropic" };
const PILOT_NOTE = "O piloto recebe o pedido inteiro e não tem tipo: ele é o padrão quando nada se encaixa.";
const PERFIL_NOTE = "Tarefas mecânicas rodam em modelo menor com menos esforço. Mudar de modelo no meio da sessão perde o cache.";
const PERFIL_SLOT: Record<string, string> = { planejar: "Planejar", implementar: "Implementar", revisar: "Revisar", testar: "Testar", explorar: "Explorar" };
/** "claude-haiku-5-5" -> "Haiku 5.5" (rótulo curto do modelo no perfil). */
export const modelLabel = (m: string) => m.replace(/^claude-/, "").split("-").map((p, i) => (i === 0 ? p[0].toUpperCase() + p.slice(1) : p)).join(" ").replace(/ (\d) (\d)$/, " $1.$2");

export default function AgenticSection() {
  const [cfg, setCfg] = useState<AgenticConfig | null>(null);
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [agents, setAgents] = useState<AgentDef[]>([]);
  const [vault, setVault] = useState<string[]>([]);
  const [showAccount, setShowAccount] = useState(false);
  const [edit, setEdit] = useState<{ index: number | null; type: TaskType } | null>(null);
  const [busy, setBusy] = useState(false);
  const [test, setTest] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [perfil, setPerfil] = useState<Perfil | null>(null);
  const settings = useStore((s) => s.settings);

  useEffect(() => {
    api.perfisGet().then((info) => setPerfil(info.editados[info.padrao] ?? info.presets.find((p) => p.id === info.padrao) ?? null)).catch(toast);
    api.agenticGet().then(setCfg).catch(toast);
    api.decisionsList(30).then(setDecisions).catch(toast);
    api.catalogAgents().then(setAgents).catch(toast);
    api.vaultNames().then(setVault).catch(toast);
  }, []);

  if (!cfg) return <><h3>Missão agêntica</h3><div className="meta">Carregando…</div></>;

  const now = Date.now();
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const todayStats = spend(decisions, today.getTime());
  const monthStats = spend(decisions, now - 30 * DAY);
  const byType = spendByType(decisions, now - 30 * DAY);
  const recent = [...decisions].sort((a, b) => b.createdAt - a.createdAt).slice(0, 100);

  async function save() {
    if (!cfg || busy) return;
    setBusy(true);
    try {
      await api.agenticSave(cfg);
      setCfg(await api.agenticGet()); // recarrega o que o backend gravou
      useStore.getState().toast("Missão agêntica salva", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }
  async function runTest() {
    setTesting(true);
    setTest(null);
    try {
      const r = await api.decisorTest();
      setTest(`Conectado, responde em ${r.latencyMs} ms · tipo: ${r.taskType} · ≈${fmtUsd(r.costUsd)} ${r.measured ? "(medido)" : "(calculado)"}`);
    } catch (e) {
      toast(e);
    } finally {
      setTesting(false);
    }
  }
  const setDecisor = (p: Partial<AgenticConfig["decisor"]>) => setCfg({ ...cfg, decisor: { ...cfg.decisor, ...p } });
  const urlErr = baseUrlError(cfg.decisor.baseUrl, cfg.decisor.kind);
  const accountsOf = (cli: string) => ["principal", ...(settings?.accounts[cli] ?? [])];

  return (
    <>
      <h3>Missão agêntica</h3>

      <h4>Quem faz cada tarefa</h4>
      <div className="row" style={{ marginBottom: 6 }}>
        <label className="row"><input type="checkbox" checked={showAccount} onChange={(e) => setShowAccount(e.target.checked)} /> Mostrar conta</label>
        <span className="spacer" />
        <button className="btn" onClick={() => setEdit({ index: null, type: { id: "", name: "", category: "", description: "", examples: [], notExamples: [], cli: "claude", model: null, effort: null, agent: "auto", account: null, skills: [], perfil: null } })}>+ Novo tipo</button>
      </div>
      <table className="tbl">
        <thead>
          <tr><th>Tipo</th><th>IA</th><th>Modelo</th><th>Quanto pensa</th><th title={PERFIL_NOTE}>Perfil</th><th>Agente</th>{showAccount && <th>Conta</th>}<th aria-label="Ações" /></tr>
        </thead>
        <tbody>
          {cfg.types.map((t, i) => (
            <tr key={t.id}>
              <td>{t.name}{i === 0 && <span className="chip" title={PILOT_NOTE}>piloto</span>}</td>
              <td>{t.cli}</td>
              <td>{t.model ?? "padrão"}</td>
              <td>{t.effort ?? "padrão"}</td>
              <td title={PERFIL_NOTE}>{perfilCell(t, perfil, () => setCfg({ ...cfg, types: cfg.types.map((x, j) => (j === i ? { ...x, model: null, effort: null } : x)) }))}</td>
              <td>{t.agent === "auto" ? "Automático" : t.agent}</td>
              {showAccount && <td>{t.account ?? "principal"}</td>}
              <td>
                <button className="icon-btn" aria-label={`Editar tipo ${t.name}`} onClick={() => setEdit({ index: i, type: { ...t, examples: [...t.examples], notExamples: [...t.notExamples], skills: [...t.skills] } })}>✎</button>
                {i > 0 && <button className="icon-btn" aria-label={`Remover tipo ${t.name}`} onClick={() => setCfg({ ...cfg, types: cfg.types.filter((_, j) => j !== i) })}>✕</button>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h4>Decisões automáticas</h4>
      <label className="row" style={{ marginBottom: 8 }}>
        <input type="checkbox" checked={cfg.chooseAgent} onChange={(e) => setCfg({ ...cfg, chooseAgent: e.target.checked })} /> Escolher o agente automaticamente
      </label>
      <div className="row" style={{ marginBottom: 8 }}>
        <div className="card" style={{ flex: 1 }}>
          <strong>Hoje</strong>
          <div className="meta">{todayStats.count} decisões · custo estimado {fmtUsd(todayStats.costUsd)} · {todayStats.errors} com erro</div>
        </div>
        <div className="card" style={{ flex: 1 }}>
          <strong>Últimos 30 dias</strong>
          <div className="meta">{monthStats.count} decisões · custo estimado {fmtUsd(monthStats.costUsd)} · {monthStats.errors} com erro</div>
        </div>
      </div>
      <p className="meta">Custo: medido quando o provedor informa; senão calculado pela tabela de preços.</p>
      <h4>Gasto de 30 dias por tipo</h4>
      {Object.keys(byType).length === 0 ? <div className="meta">Nenhuma decisão nos últimos 30 dias.</div> : (
        <table className="tbl">
          <tbody>
            {Object.entries(byType).map(([id, s]) => (
              <tr key={id}><td>{id === "(sem tipo)" ? id : typeName(cfg.types, id)}</td><td>{s.count}</td><td>{fmtUsd(s.costUsd)}</td><td>{s.errors ? `${s.errors} erro(s)` : ""}</td></tr>
            ))}
          </tbody>
        </table>
      )}
      <h4>Últimas decisões</h4>
      {recent.length === 0 ? <div className="meta">Nenhuma decisão ainda.</div> : (
        <div style={{ maxHeight: 300, overflow: "auto" }}>
          <table className="tbl">
            <thead><tr><th>Quando</th><th>Pedido</th><th>Tipo</th><th>IA / modelo</th><th>Agente</th><th>Decidiu</th><th>Custo</th></tr></thead>
            <tbody>
              {recent.map((d) => (
                <tr key={d.id} title={d.receipt}>
                  <td>{new Date(d.createdAt).toLocaleString("pt-BR")}</td>
                  <td>{short(d.request)}</td>
                  <td>{typeName(cfg.types, d.taskType)}</td>
                  <td>{[d.cli, d.model].filter(Boolean).join(" / ") || "—"}</td>
                  <td>{d.agent ?? "—"}</td>
                  <td>{d.decidedBy}</td>
                  <td>{fmtUsd(d.costUsd)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <h4>Modelo de decisão</h4>
      <div className="card">
        <label className="field"><span>Tipo</span>
          <select value={cfg.decisor.kind} onChange={(e) => { const kind = e.target.value as AgenticConfig["decisor"]["kind"]; setDecisor({ kind, ...(kind === "none" ? { baseUrl: "" } : {}) }); }}>
            {Object.entries(KIND).map(([id, label]) => <option key={id} value={id}>{label}</option>)}
          </select>
        </label>
        {cfg.decisor.kind !== "none" && (
          <>
            <label className="field"><span>URL base</span>
              <input value={cfg.decisor.baseUrl} placeholder="https://openrouter.ai/api/v1" aria-invalid={!!urlErr} onChange={(e) => setDecisor({ baseUrl: e.target.value })} />
              <span className="meta">{urlErr ?? "https; http só para localhost."}</span>
            </label>
            <label className="field"><span>Modelo</span><input value={cfg.decisor.model} placeholder="ex.: haiku-5-5" onChange={(e) => setDecisor({ model: e.target.value })} /></label>
            <label className="field"><span>Chave no Cofre</span>
              <select value={cfg.decisor.secret ?? ""} onChange={(e) => setDecisor({ secret: e.target.value || null })}>
                <option value="">nenhuma</option>
                {vault.map((n) => <option key={n} value={n}>{n}</option>)}
              </select>
              <span className="meta">A chave fica no Cofre (seção Cofre), pelo nome. Nunca aparece aqui.</span>
            </label>
            <p className="meta">O texto do pedido é enviado ao provedor configurado.</p>
            <div className="row">
              <button className="btn" disabled={testing || !!urlErr} onClick={runTest}>Testar</button>
              {test && <span aria-live="polite">{test}</span>}
            </div>
          </>
        )}
      </div>

      <div className="actions">
        <button className="btn primary" disabled={busy || (cfg.decisor.kind !== "none" && !!urlErr)} onClick={save}>Salvar</button>
      </div>

      {edit && (
        <TypeDialog
          index={edit.index}
          initial={edit.type}
          taken={cfg.types.map((t) => t.id).filter((id) => id !== edit.type.id)}
          agents={agents}
          accountsOf={accountsOf}
          onClose={() => setEdit(null)}
          onSave={(t) => {
            setCfg({ ...cfg, types: edit.index === null ? [...cfg.types, t] : cfg.types.map((x, j) => (j === edit.index ? t : x)) });
            setEdit(null);
          }}
        />
      )}
    </>
  );
}

/** Coluna "Perfil": segue o slot do perfil padrão ou fica personalizada, com o botão para voltar a seguir o perfil. */
function perfilCell(t: TaskType, perfil: Perfil | null, reset: () => void) {
  if (!t.perfil || t.cli !== "claude") return <span className="meta">—</span>;
  const custom = t.model !== null || t.effort !== null;
  if (custom) {
    return <>personalizado <button className="btn" onClick={reset}>Voltar a seguir o perfil</button></>;
  }
  const slot = perfil?.modelos[t.perfil as keyof Perfil["modelos"]];
  const nome = PERFIL_SLOT[t.perfil] ?? t.perfil;
  return <>{slot ? `segue o perfil (${modelLabel(slot.model)} · ${slot.effort})` : `segue o perfil (${nome})`}</>;
}

function TypeDialog({ index, initial, taken, agents, accountsOf, onClose, onSave }: {
  index: number | null; initial: TaskType; taken: string[]; agents: AgentDef[]; accountsOf(cli: string): string[];
  onClose(): void; onSave(t: TaskType): void;
}) {
  const [t, setT] = useState<TaskType>(initial);
  const [examples, setExamples] = useState(listToLines(initial.examples));
  const [notExamples, setNotExamples] = useState(listToLines(initial.notExamples));
  const [skills, setSkills] = useState<SkillDef[]>([]);
  const providers = useStore((s) => s.providers);
  const cli = providers?.clis.find((c) => c.id === t.cli);
  useEffect(() => { api.catalogSkills().then(setSkills).catch(toast); }, []);
  const set = (p: Partial<TaskType>) => setT((x) => ({ ...x, ...p }));

  function done() {
    if (!t.name.trim()) return useStore.getState().toast("dê um nome ao tipo", "erro");
    const id = index === null ? slugId(t.name, taken) : t.id;
    onSave({ ...t, id, name: t.name.trim(), examples: linesToList(examples), notExamples: linesToList(notExamples), model: t.model || null, effort: t.effort || null, account: t.account || null });
  }

  return (
    <Modal label={index === null ? "Novo tipo de tarefa" : "Editar tipo de tarefa"} onClose={onClose}>
      <form onSubmit={(e) => { e.preventDefault(); done(); }}>
        <h2>{index === null ? "Novo tipo de tarefa" : `Editar: ${t.name}`}</h2>
        <label className="field"><span>Nome</span><input value={t.name} onChange={(e) => set({ name: e.target.value })} /></label>
        <label className="field"><span>Descrição</span><input value={t.description} onChange={(e) => set({ description: e.target.value })} /></label>
        <label className="field"><span>Exemplos (um por linha)</span><textarea rows={3} value={examples} onChange={(e) => setExamples(e.target.value)} /></label>
        <label className="field"><span>O que não é deste tipo (um por linha)</span><textarea rows={2} value={notExamples} onChange={(e) => setNotExamples(e.target.value)} /></label>
        <div className="row">
          <label className="field"><span>IA</span>
            <select value={t.cli} onChange={(e) => set({ cli: e.target.value, model: null, effort: null, account: null })}>
              {(providers?.clis ?? []).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
            </select>
          </label>
          <label className="field"><span>Modelo</span><input list="agentic-models" value={t.model ?? ""} placeholder="padrão" onChange={(e) => set({ model: e.target.value })} />
            <datalist id="agentic-models">{(cli?.models ?? []).map((m) => <option key={m} value={m} />)}</datalist>
          </label>
          <label className="field"><span>Quanto pensa</span>
            <select value={t.effort ?? ""} onChange={(e) => set({ effort: e.target.value || null })}>
              <option value="">padrão</option>
              {(cli?.efforts ?? []).map((x) => <option key={x} value={x}>{x}</option>)}
            </select>
          </label>
          <label className="field"><span>Conta</span>
            <select value={t.account ?? "principal"} onChange={(e) => set({ account: e.target.value === "principal" ? null : e.target.value })}>
              {accountsOf(t.cli).map((a) => <option key={a} value={a}>{a}</option>)}
            </select>
          </label>
        </div>
        <label className="field"><span>Agente</span>
          <select value={t.agent} onChange={(e) => set({ agent: e.target.value })}>
            <option value="auto">Automático</option>
            {agents.map((a) => <option key={a.name} value={a.name}>{a.name}</option>)}
          </select>
        </label>
        <fieldset className="field" style={{ border: 0, padding: 0 }}>
          <legend>Skills</legend>
          {skills.length === 0 && <span className="meta">Nenhuma skill instalada.</span>}
          <div className="row">
            {skills.map((s) => (
              <label key={s.name} className="chip">
                <input type="checkbox" checked={t.skills.includes(s.name)} onChange={(e) => set({ skills: e.target.checked ? [...t.skills, s.name] : t.skills.filter((x) => x !== s.name) })} />
                {s.name}
              </label>
            ))}
          </div>
        </fieldset>
        <div className="actions">
          <button type="button" className="btn" onClick={onClose}>Cancelar</button>
          <button type="submit" className="btn primary">Aplicar</button>
        </div>
      </form>
    </Modal>
  );
}
