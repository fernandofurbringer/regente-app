import { useEffect, useRef, useState } from "react";
import { api, errMsg, type RecipeDef, type SquadDef } from "../lib/ipc";
import { defaultAgentCli } from "../lib/quickstart";
import { readLastCli, readMission, rememberLastCli, rememberMission } from "../lib/lastAgent";
import {
  applyRecipe, autofill, briefFromIssue, chosenSlots, initialWizard, memoryOf, missingFor, missionInput, pickSquad, presetBuilderReviewer, presetCompare, setBrief, withMemory, type WizardMode, type WizardState,
} from "../lib/wizard";
import { useStore, wsOf } from "../state/store";
import BriefAssist from "./BriefAssist";
import Modal from "./Modal";
import { SlotFields, defaultSlot, type SlotSpec } from "./NewPaneDialog";
import { PapeisDoProjeto } from "./PapeisTable";

const MODES: { id: WizardMode; label: string; hint: string }[] = [
  { id: "livre", label: "Livre", hint: "você comanda os terminais" },
  { id: "squad", label: "Squad", hint: "um time com maestro, fases e gates" },
  { id: "agentico", label: "Agêntico", hint: "um piloto decide e delega" },
];
const STYLES: { id: "chain" | "torre" | "plano" | "paralelo"; label: string; hint: string }[] = [
  { id: "chain", label: "Chain", hint: "scout → builder → reviewer, em sequência" },
  { id: "paralelo", label: "Paralelo", hint: "o piloto quebra em tarefas e roda ao mesmo tempo as que não dependem de outra; revisão no fim" },
  { id: "torre", label: "Torre de controle", hint: "um controlador exclusivo delega e registra tudo" },
  { id: "plano", label: "Plano → Execução", hint: "Um planejador estuda e escreve o plano; você aprova; executores rápidos fazem cada tarefa e o Regente verifica." },
];
const START: Record<WizardMode, string> = { livre: "Iniciar Missão", squad: "Iniciar Squad", agentico: "Iniciar Agêntico" };

export default function NewMissionWizard({ wsId }: { wsId: string }) {
  // começa com a última missão (modo, terminais, terminal padrão, worktree) e a última IA escolhida
  const [st, setSt] = useState<WizardState>(() => withMemory(initialWizard(defaultSlot(defaultAgentCli(useStore.getState().detected, readLastCli()) ?? "claude"), "chain"), readMission(),
    (cli) => useStore.getState().detected.some((d) => d.kind === "cli" && d.id === cli && !!d.path)));
  // o usuário mexeu no checkbox de worktree (só possível em repositório): a leitura assíncrona do git não sobrescreve
  const worktreeTouched = useRef(false);
  const [recipes, setRecipes] = useState<RecipeDef[] | null>(null);
  const [squads, setSquads] = useState<SquadDef[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [issueOpen, setIssueOpen] = useState(false);
  const [issueRef, setIssueRef] = useState("");
  const [issueBusy, setIssueBusy] = useState(false);
  // a pasta é um repositório git? null enquanto verifica
  const [inRepo, setInRepo] = useState<boolean | null>(null);
  const providers = useStore((s) => s.providers);
  const detected = useStore((s) => s.detected);
  const { patch, toast, loadMissions, selectMission } = useStore.getState();
  const close = () => patch({ dialog: null });
  const upd = (p: Partial<WizardState>) => setSt((s) => ({ ...s, ...p }));
  const chosen = chosenSlots(st);
  const agentSlots = st.mode === "livre" ? chosen : [];
  const pilotCli = st.mode === "agentico" ? st.pilot.cli : null;
  const missing = [...new Set([...agentSlots.filter((s) => s.kind === "agent").map((s) => s.cli), pilotCli]
    .filter((c): c is string => !!c && !detected.some((d) => d.kind === "cli" && d.id === c && d.path)))]
    .map((id) => providers?.clis.find((c) => c.id === id)?.label ?? id);
  const squad = squads?.find((s) => s.id === st.squadId);
  const reason = missingFor(st);

  // vindo da paleta com um brief: aplica uma vez (o store é limpo para não voltar na próxima abertura)
  useEffect(() => {
    const pre = useStore.getState().missionPrefill;
    if (!pre) return;
    setSt((s) => ({ ...s, mode: pre.mode, brief: pre.brief, ...(pre.style ? { style: pre.style } : {}) }));
    patch({ missionPrefill: null });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    // em repositório, o worktree já vem ligado (sem receita, que decide o dela); fora, desligado
    // a última missão, se houver, decide o worktree; sem ela, em repositório ele vem ligado
    const mem = readMission();
    api.gitIsRepo(wsId).then((repo) => {
      setInRepo(repo);
      if (worktreeTouched.current) return;
      setSt((s) => ({ ...s, worktree: !repo ? false : s.recipeId !== null ? s.worktree : (mem?.worktree ?? true) }));
    }).catch(() => {
      setInRepo(false);
      setSt((s) => ({ ...s, worktree: false }));
    });
    api.providersDetect().then((d) => patch({ detected: d })).catch((e) => toast(errMsg(e), "erro"));
    api.recipesList().then(setRecipes).catch((e) => { setRecipes([]); toast(errMsg(e), "erro"); });
    api.squadsList().then(setSquads).catch((e) => { setSquads([]); toast(errMsg(e), "erro"); });
    api.agenticGet().then((c) => setSt((s) => ({ ...s, style: c.defaultStyle }))).catch((e) => toast(errMsg(e), "erro"));
  }, [patch, toast, wsId]);

  const step = (d: number) => setSt((s) => ({ ...s, count: Math.min(64, Math.max(1, s.count + d)) }));

  function applyChosenRecipe(id: string) {
    if (!id) return upd({ recipeId: null });
    const r = recipes?.find((x) => x.id === id);
    if (!r) return;
    setSt((s) => applyRecipe(r, s, squads?.find((x) => x.id === r.squad)));
  }

  async function start() {
    const ws = wsOf(wsId);
    if (!ws || busy || reason) return;
    setBusy(true);
    try {
      // fora de repositório (ou se o git não respondeu) não há worktree, mesmo que o checkbox tenha ficado marcado
      const m = await api.missionCreate(missionInput(inRepo === true ? st : { ...st, worktree: false }, { workspaceId: ws.id, cwd: ws.path }));
      // o worktree só é lembrado quando o usuário o escolheu num repositório; senão mantém a memória anterior
      rememberMission({ ...memoryOf(st), worktree: worktreeTouched.current ? st.worktree : (readMission()?.worktree ?? st.worktree) });
      if (st.template.kind === "agent" && st.template.cli) rememberLastCli(st.template.cli);
      await loadMissions(ws.id);
      selectMission(ws.id, m.id);
      close();
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  const intakeAuto = squad ? autofill(squad.intake, st.brief) : {};

  // o brief de squad passa pelo intake (respostas automáticas); o de agêntico é o texto puro
  const takeBrief = (text: string, name?: string) => setSt((s) => ({ ...setBrief(s, squad?.intake ?? [], text), ...(name !== undefined ? { name } : {}) }));

  async function fromClipboard() {
    let text = "";
    try {
      text = (await navigator.clipboard.readText()).trim();
    } catch {
      return toast("Não foi possível ler a área de transferência.", "erro");
    }
    if (!text) return toast("A área de transferência está vazia.", "erro");
    takeBrief(text);
  }

  // `gh` só roda aqui, no clique; a issue vira nome e brief
  async function fromIssue() {
    const ws = wsOf(wsId);
    if (!ws || issueBusy || !issueRef.trim()) return;
    setIssueBusy(true);
    try {
      const { name, brief } = briefFromIssue(await api.ghIssue(ws.id, issueRef.trim()));
      takeBrief(brief, name);
      setIssueOpen(false);
      setIssueRef("");
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setIssueBusy(false);
    }
  }

  return (
    <Modal label="Nova missão" onClose={close} autoFocus={false}>
      {/* Ctrl+Enter inicia de qualquer campo; Enter sozinho segue o comportamento do campo */}
      <form onSubmit={(e) => { e.preventDefault(); start(); }}
        onKeyDown={(e) => { if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); void start(); } }}>
        <h2>Nova missão</h2>
        <div className="field">
          <span>Modo</span>
          <div className="mode-cards" role="group" aria-label="Modo da missão">
            {MODES.map((m) => (
              <button key={m.id} type="button" className="mode-card" aria-pressed={st.mode === m.id} onClick={() => upd({ mode: m.id })}>
                <strong>{m.label}</strong>
                <small>{m.hint}</small>
              </button>
            ))}
          </div>
        </div>
        <label className="field">
          <span>Usar receita</span>
          {recipes === null ? <span className="meta">Carregando receitas…</span>
            : recipes.length === 0 ? <span className="meta">Nenhuma receita ainda.</span>
            : (
              <select value={st.recipeId ?? ""} onChange={(e) => applyChosenRecipe(e.target.value)}>
                <option value="">Nenhuma</option>
                {recipes.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
              </select>
            )}
        </label>
        <label className="field"><span>Nome</span><input autoFocus value={st.name} placeholder="ex.: landing page nova" onChange={(e) => upd({ name: e.target.value })} /></label>
        {st.mode !== "livre" && (
          <div className="field" role="group" aria-label="Preencher o pedido da missão">
            <div className="row" style={{ gap: 6, flexWrap: "wrap" }}>
              <button type="button" className="btn" onClick={() => void fromClipboard()}>Usar área de transferência</button>
              <button type="button" className="btn" aria-expanded={issueOpen} onClick={() => setIssueOpen((o) => !o)}>De uma issue</button>
            </div>
            {issueOpen && (
              <div className="row" style={{ gap: 6, marginTop: 6 }}>
                <input aria-label="Número ou URL da issue" placeholder="123, #123 ou URL da issue" value={issueRef} style={{ flex: 1 }}
                  onChange={(e) => setIssueRef(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); void fromIssue(); } }} />
                <button type="button" className="btn" disabled={!issueRef.trim() || issueBusy} onClick={() => void fromIssue()}>{issueBusy ? "Buscando…" : "Buscar"}</button>
              </div>
            )}
            <span className="meta">A issue é lida pelo GitHub CLI (gh) desta pasta, se estiver instalado e autenticado.</span>
          </div>
        )}
        <label className="row" style={{ marginBottom: 10 }}>
          <input type="checkbox" checked={st.worktree} disabled={inRepo !== true} onChange={(e) => { worktreeTouched.current = true; upd({ worktree: e.target.checked }); }} /> Isolar em worktree (recomendado com 2+ agentes)
        </label>
        {inRepo === false && <p className="meta" style={{ marginTop: -6 }}>Worktree indisponível: esta pasta não é um repositório git.</p>}
        <label className="field">
          <span>Perfil desta missão</span>
          <select value={st.profile ?? ""} onChange={(e) => upd({ profile: e.target.value || null })}>
            <option value="">Do projeto/padrão</option>
            <option value="economico">Econômico</option>
            <option value="equilibrado">Equilibrado</option>
            <option value="precisao">Máxima precisão</option>
          </select>
        </label>

        {st.mode === "livre" && (
          <>
            <div className="field">
              <span>Terminais</span>
              <div className="row" role="group" aria-label="Número de terminais" style={{ gap: 6 }}
                onKeyDown={(e) => { if (e.key === "ArrowUp" || e.key === "ArrowDown") { e.preventDefault(); step(e.key === "ArrowUp" ? 1 : -1); } }}>
                <button type="button" className="btn" aria-label="Um terminal a menos" aria-disabled={st.count <= 1} onClick={() => st.count > 1 && step(-1)}>−</button>
                <output aria-live="polite" style={{ minWidth: 28, textAlign: "center" }}>{st.count}</output>
                <button type="button" className="btn" aria-label="Um terminal a mais" aria-disabled={st.count >= 64} onClick={() => st.count < 64 && step(1)}>+</button>
              </div>
            </div>
            <div className="row" role="group" aria-label="Predefinições" style={{ gap: 6, flexWrap: "wrap", marginBottom: 10 }}>
              <button type="button" className="btn" onClick={() => setSt((s) => presetCompare(s))}>Comparar 3 modelos</button>
              <button type="button" className="btn" onClick={() => setSt((s) => presetBuilderReviewer(s))}>Builder + reviewer</button>
            </div>
            <label className="field">
              <span>Pedido inicial (opcional)</span>
              <textarea rows={3} value={st.prompt} onChange={(e) => upd({ prompt: e.target.value })} />
            </label>
            <label className="row">
              <input type="checkbox" checked={st.sendToAll} onChange={(e) => upd({ sendToAll: e.target.checked })} /> Enviar este pedido a todos os terminais de IA
            </label>
            <p className="meta">Cole a tarefa uma vez; os agentes começam juntos para você comparar.</p>
            <div className="field">
              <span>{st.individual ? "Padrão" : "Configuração aplicada a todos"}</span>
              <SlotFields value={st.template} cwd={wsOf(wsId)?.path ?? null} onChange={(template) => upd({ template })} />
            </div>
            <label className="row">
              <input type="checkbox" checked={st.individual} onChange={(e) => upd({ individual: e.target.checked })} /> Configurar cada terminal
            </label>
            {st.individual && (
              <div style={{ maxHeight: 280, overflow: "auto", marginTop: 8 }}>
                {Array.from({ length: st.count }, (_, i) => (
                  <div key={i} className="card">
                    <strong>#{i + 1}</strong>
                    <SlotFields value={st.slots[i] ?? st.template} cwd={wsOf(wsId)?.path ?? null} onChange={(v: SlotSpec) => setSt((s) => { const slots = [...s.slots]; slots[i] = v; return { ...s, slots }; })} />
                    {(st.slots[i] ?? st.template).kind === "agent" && (
                      <label className="field">
                        <span>Pedido inicial</span>
                        <textarea rows={2} value={st.slots[i]?.prompt ?? (st.sendToAll ? st.prompt : "")}
                          onChange={(e) => setSt((s) => { const slots = [...s.slots]; slots[i] = { ...(s.slots[i] ?? s.template), prompt: e.target.value }; return { ...s, slots }; })} />
                      </label>
                    )}
                  </div>
                ))}
              </div>
            )}
          </>
        )}

        {st.mode === "squad" && (
          <>
            <div className="field">
              <span>Squad</span>
              {squads === null ? <span className="meta">Carregando squads…</span>
                : squads.length === 0 ? <span className="meta">Nenhuma squad instalada.</span>
                : (
                  <div className="squad-grid" role="group" aria-label="Squads">
                    {squads.map((s) => (
                      <button key={s.id} type="button" className="mode-card" aria-pressed={st.squadId === s.id} onClick={() => setSt((x) => pickSquad(x, s))}>
                        <strong>{s.name}</strong>
                        <small>{s.description}</small>
                        <small>pico de {s.peakPanes ?? "—"} painéis · {s.phases.filter((p) => p.gate).length} gates</small>
                      </button>
                    ))}
                  </div>
                )}
            </div>
            {squad && (
              <>
                <label className="field">
                  <span>Pedido da missão</span>
                  <textarea rows={4} value={st.brief} placeholder="o que construir" onChange={(e) => setSt((s) => setBrief(s, squad.intake, e.target.value))} />
                </label>
                <BriefAssist brief={st.brief} onChange={(text) => setSt((s) => setBrief(s, squad.intake, text))} />
                <label className="row" style={{ marginBottom: 10 }}>
                  <input type="checkbox" checked={st.intakeHidden} onChange={(e) => upd({ intakeHidden: e.target.checked })} /> Já tenho tudo definido
                </label>
                {!st.intakeHidden && squad.intake.map((q, i) => {
                  const auto = intakeAuto[i];
                  const filled = auto !== undefined && st.answers[i] === auto;
                  return (
                    <label key={i} className="field">
                      <span>{q.q} {filled && <span className="chip">preenchido do pedido</span>}</span>
                      <input value={st.answers[i] ?? ""} onChange={(e) => setSt((s) => ({ ...s, answers: { ...s.answers, [i]: e.target.value } }))} />
                    </label>
                  );
                })}
              </>
            )}
          </>
        )}

        {st.mode === "agentico" && (
          <>
            <div className="field">
              <span>Piloto</span>
              <SlotFields pilotOnly value={{ kind: "agent", cli: st.pilot.cli, model: st.pilot.model, effort: st.pilot.effort, account: st.pilot.account }}
                onChange={(v) => upd({ pilot: { cli: v.cli ?? null, model: v.model ?? null, effort: v.effort ?? null, account: v.account ?? null } })} />
            </div>
            <div className="field">
              <span>Estilo</span>
              <div className="mode-cards" role="group" aria-label="Estilo da missão">
                {STYLES.map((s) => (
                  <button key={s.id} type="button" className="mode-card" aria-pressed={st.style === s.id} onClick={() => upd({ style: s.id })}>
                    <strong>{s.label}</strong>
                    <small>{s.hint}</small>
                  </button>
                ))}
              </div>
            </div>
            <PapeisDoProjeto wsId={wsId} profile={st.profile ?? null} />
            <label className="field">
              <span>Pedido da missão</span>
              <textarea rows={4} value={st.brief} placeholder="o que o piloto deve conseguir" onChange={(e) => upd({ brief: e.target.value })} />
            </label>
            <BriefAssist brief={st.brief} onChange={(text) => upd({ brief: text })} />
          </>
        )}

        {missing.length > 0 && <div className="card" role="alert">Não instalado neste computador: {missing.join(", ")}. Instale em Configurações → Suas IAs.</div>}
        <div className="actions">
          {reason && <span className="meta" style={{ marginRight: "auto", alignSelf: "center" }}>{reason}</span>}
          <button type="button" className="btn" onClick={close}>Cancelar</button>
          <button type="submit" className="btn primary" disabled={busy || !!reason}>{START[st.mode]}</button>
        </div>
      </form>
    </Modal>
  );
}
