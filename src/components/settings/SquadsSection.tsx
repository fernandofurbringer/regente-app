import { useEffect, useState } from "react";
import { copyId, gateCount, squadFormerPrompt, stepsToText, textToSteps } from "../../lib/squads";
import { api, errMsg, type AgentDef, type IntakeQ, type SquadRoster, type SquadDef, type SquadPhase } from "../../lib/ipc";
import { useStore, wsOf } from "../../state/store";
import { spawnInActive } from "./Settings";
import Modal from "../Modal";

const SOURCE: Record<string, string> = { original: "original", regente: "Regente" };
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const blank = (): SquadDef => ({
  id: "", name: "", description: "", intake: [], roster: [], phases: [], maestroNotes: "", maestro: null, peakPanes: null, source: "regente",
});
const emptyRoster = (): SquadRoster => ({ agent: "", role: null, cli: null, model: null, effort: null, max: null });

export default function SquadsSection() {
  const [squads, setSquads] = useState<SquadDef[] | null>(null);
  const [agents, setAgents] = useState<AgentDef[]>([]);
  const [sel, setSel] = useState<string | null>(null);
  const [draft, setDraft] = useState<SquadDef | null>(null);
  const [busy, setBusy] = useState(false);
  /** o formulário cria uma squad nova (id editável e não pode repetir um existente) */
  const [creating, setCreating] = useState(false);
  const [askFormer, setAskFormer] = useState(false);
  const active = useStore((s) => s.active);
  const providers = useStore((s) => s.providers);

  const load = () => api.squadsList().then((l) => { setSquads(l); return l; }).catch((e) => { setSquads([]); toast(e); return [] as SquadDef[]; });
  useEffect(() => {
    load();
    api.catalogAgents().then(setAgents).catch(toast);
  }, []);

  function open(s: SquadDef) {
    setSel(s.id);
    setCreating(false);
    setDraft(JSON.parse(JSON.stringify(s)) as SquadDef);
  }
  function duplicate(s: SquadDef) {
    const d = { ...(JSON.parse(JSON.stringify(s)) as SquadDef), id: copyId(s.id, (squads ?? []).map((x) => x.id)), source: "regente" as const };
    setSel(null);
    setCreating(true);
    setDraft(d);
  }
  async function save() {
    if (!draft || busy) return;
    if (creating && exists) return useStore.getState().toast(`já existe uma squad com o id "${draft.id}"`, "erro");
    setBusy(true);
    try {
      await api.squadSave(draft);
      useStore.getState().toast("Squad salva", "ok");
      const list = await load();
      setSel(draft.id);
      setCreating(false);
      setDraft(list.find((x) => x.id === draft.id) ?? null);
    } catch (e) {
      toast(e); // ex.: bad_cli — "o maestro precisa do Claude Code"
    } finally {
      setBusy(false);
    }
  }
  async function former() {
    const ws = wsOf(active);
    if (!ws) return useStore.getState().toast("Abra um workspace primeiro.", "erro");
    try {
      const panes = await api.squadFormerBrief();
      await spawnInActive({ kind: "agent", cli: "claude", model: "claude-opus-5-5", effort: "high", role: "maestro", mcpProfile: "minimal", name: "Squad Former", prompt: squadFormerPrompt(panes) });
    } catch (e) {
      toast(e);
    }
  }

  async function confirmFormer() {
    setAskFormer(false);
    await former();
  }

  const readonly = draft?.source === "original";
  const exists = draft !== null && (squads ?? []).some((s) => s.id === draft.id);

  return (
    <>
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 10 }}>
        <h3 style={{ margin: 0 }}>Squads</h3>
        <div className="row">
          <button className="btn" onClick={() => setAskFormer(true)} title="Analisa as 20 sessões recentes e propõe um squad">Squad Former</button>
          <button className="btn primary" onClick={() => { setSel(null); setCreating(true); setDraft(blank()); }}>Nova squad</button>
        </div>
      </div>
      {askFormer && (
        <Modal label="Abrir o Squad Former" onClose={() => setAskFormer(false)}>
          <p>Abrir o Squad Former? Ele usa Claude Opus (custo maior).</p>
          <div className="actions">
            <button className="btn" onClick={() => setAskFormer(false)}>Cancelar</button>
            <button className="btn primary" onClick={confirmFormer}>Abrir</button>
          </div>
        </Modal>
      )}
      <div className="row" style={{ alignItems: "flex-start" }}>
        <div style={{ flex: "1 1 200px" }}>
          {squads === null ? <div className="meta">Carregando squads…</div>
            : squads.length === 0 ? <div className="meta">Nenhuma squad ainda.</div>
            : (
              <ul style={{ listStyle: "none", margin: 0, padding: 0, display: "grid", gap: 4 }} aria-label="Squads">
                {squads.map((s) => (
                  <li key={s.id}>
                    <button className={`rail-item ${sel === s.id ? "sel" : ""}`} aria-current={sel === s.id} onClick={() => open(s)}>
                      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{s.name}</span>
                      <span className="meta">{SOURCE[s.source] ?? s.source}</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
        </div>
        {draft && (
          <div className="card" style={{ flex: "3 1 420px", minWidth: 0 }}>
            {readonly && (
              <div className="row" style={{ justifyContent: "space-between" }}>
                <span className="meta">Original: somente leitura.</span>
                <button className="btn" onClick={() => duplicate(draft)}>Duplicar para editar</button>
              </div>
            )}
            <fieldset disabled={readonly} style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
              <div className="row">
                <label className="field"><span>Id</span><input value={draft.id} disabled={!creating} placeholder="ex.: jogo-web-neon" onChange={(e) => setDraft({ ...draft, id: e.target.value })} /></label>
                <label className="field" style={{ flex: 1 }}><span>Nome</span><input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></label>
              </div>
              <label className="field"><span>Descrição</span><input value={draft.description} onChange={(e) => setDraft({ ...draft, description: e.target.value })} /></label>

              <h4>Intake (perguntas do brief)</h4>
              {draft.intake.map((q, i) => (
                <div key={i} className="row">
                  <input aria-label={`Pergunta ${i + 1}`} value={q.q} placeholder="ex.: URL de referência" style={{ flex: 1 }}
                    onChange={(e) => setDraft({ ...draft, intake: draft.intake.map((x, j) => (j === i ? { ...x, q: e.target.value } : x)) })} />
                  <select aria-label={`Preenchimento de ${i + 1}`} value={q.autofill ?? ""} onChange={(e) => setDraft({ ...draft, intake: draft.intake.map((x, j) => (j === i ? { ...x, autofill: (e.target.value || null) as IntakeQ["autofill"] } : x)) })}>
                    <option value="">sem preenchimento</option>
                    <option value="first_url">primeira URL do brief</option>
                    <option value="inspired_by">"inspirado em" do brief</option>
                  </select>
                  <button className="icon-btn" aria-label={`Remover pergunta ${i + 1}`} onClick={() => setDraft({ ...draft, intake: draft.intake.filter((_, j) => j !== i) })}>✕</button>
                </div>
              ))}
              <button className="btn" onClick={() => setDraft({ ...draft, intake: [...draft.intake, { q: "", autofill: null }] })}>+ Pergunta</button>

              <h4>Time (roster)</h4>
              {draft.roster.map((r, i) => (
                <div key={i} className="row">
                  <select aria-label={`Agente ${i + 1}`} value={r.agent} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, agent: e.target.value } : x)) })}>
                    <option value="">escolha um agente</option>
                    {agents.map((a) => <option key={a.name} value={a.name}>{a.name}</option>)}
                  </select>
                  <input aria-label={`Função ${i + 1}`} placeholder="função" value={r.role ?? ""} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, role: e.target.value || null } : x)) })} />
                  <select aria-label={`IA ${i + 1}`} value={r.cli ?? ""} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, cli: e.target.value || null } : x)) })}>
                    <option value="">IA do agente</option>
                    {(providers?.clis ?? []).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
                  </select>
                  <input aria-label={`Modelo ${i + 1}`} placeholder="modelo" value={r.model ?? ""} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, model: e.target.value || null } : x)) })} />
                  <input aria-label={`Esforço ${i + 1}`} placeholder="esforço" value={r.effort ?? ""} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, effort: e.target.value || null } : x)) })} />
                  <input aria-label={`Máximo ${i + 1}`} type="number" min={1} style={{ width: 70 }} placeholder="máx." value={r.max ?? ""} onChange={(e) => setDraft({ ...draft, roster: draft.roster.map((x, j) => (j === i ? { ...x, max: e.target.value ? Number(e.target.value) : null } : x)) })} />
                  <button className="icon-btn" aria-label={`Remover agente ${i + 1}`} onClick={() => setDraft({ ...draft, roster: draft.roster.filter((_, j) => j !== i) })}>✕</button>
                </div>
              ))}
              <button className="btn" onClick={() => setDraft({ ...draft, roster: [...draft.roster, emptyRoster()] })}>+ Agente</button>

              <h4>Fases</h4>
              {draft.phases.map((p, i) => (
                <div key={i} className="card">
                  <div className="row">
                    <input aria-label={`Nome da fase ${i + 1}`} placeholder="nome da fase" value={p.name} style={{ flex: 1 }} onChange={(e) => setPhase(i, { name: e.target.value })} />
                    <input aria-label={`Gate da fase ${i + 1}`} placeholder="título do gate (opcional)" value={p.gate ?? ""} onChange={(e) => setPhase(i, { gate: e.target.value || null })} />
                    <button className="icon-btn" aria-label={`Remover fase ${i + 1}`} onClick={() => setDraft({ ...draft, phases: draft.phases.filter((_, j) => j !== i) })}>✕</button>
                  </div>
                  <StepsField label={`Passos da fase ${i + 1}`} steps={p.steps} onCommit={(steps) => setPhase(i, { steps })} />
                </div>
              ))}
              <button className="btn" onClick={() => setDraft({ ...draft, phases: [...draft.phases, { name: "", steps: [], gate: null }] })}>+ Fase</button>

              <h4>Maestro</h4>
              <p className="meta">O maestro é sempre Claude Code.</p>
              <div className="row">
                <label className="field"><span>Modelo</span><input value={draft.maestro?.model ?? ""} placeholder="padrão (opus)" onChange={(e) => setMaestro({ model: e.target.value || null })} /></label>
                <label className="field"><span>Quanto pensa</span><input value={draft.maestro?.effort ?? ""} placeholder="padrão (high)" onChange={(e) => setMaestro({ effort: e.target.value || null })} /></label>
              </div>
              <label className="field"><span>Notas para o maestro</span><textarea rows={3} value={draft.maestroNotes} onChange={(e) => setDraft({ ...draft, maestroNotes: e.target.value })} /></label>
              <span className="meta">{gateCount(draft)} gate(s)</span>
            </fieldset>
            {!readonly && (
              <div className="actions">
                <button className="btn primary" disabled={busy || !draft.id.trim() || !draft.name.trim() || (creating && exists)} onClick={save}>Salvar</button>
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );

  function setPhase(i: number, p: Partial<SquadPhase>) {
    if (!draft) return;
    setDraft({ ...draft, phases: draft.phases.map((x, j) => (j === i ? { ...x, ...p } : x)) });
  }
  function setMaestro(p: { model?: string | null; effort?: string | null }) {
    if (!draft) return;
    const m: SquadRoster = { ...(draft.maestro ?? emptyRoster()), agent: "maestro", cli: "claude", ...p };
    setDraft({ ...draft, maestro: m.model || m.effort ? m : null });
  }
}

/** Passos de uma fase: texto livre enquanto digita (linhas vazias ficam), normalizado ao sair do campo. */
function StepsField({ label, steps, onCommit }: { label: string; steps: string[]; onCommit(steps: string[]): void }) {
  const [text, setText] = useState(stepsToText(steps));
  useEffect(() => setText(stepsToText(steps)), [steps]);
  return (
    <textarea aria-label={label} rows={3} placeholder="um passo por linha" value={text}
      onChange={(e) => setText(e.target.value)} onBlur={() => onCommit(textToSteps(text))} />
  );
}
