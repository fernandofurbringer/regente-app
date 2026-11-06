import type { MissionMemory } from "./lastAgent";
import type { GhIssue, IntakeQ, NewMission, PaneSpec, RecipeDef, SquadDef } from "./ipc";

export type WizardMode = "livre" | "squad" | "agentico";
export type SlotSpec = Omit<PaneSpec, "workspaceId" | "cwd">;
export interface Pilot { cli: string | null; model: string | null; effort: string | null; account: string | null }

export interface WizardState {
  mode: WizardMode;
  recipeId: string | null;
  name: string;
  worktree: boolean;
  /** livre: quantos terminais, o padrão de todos e as configurações por terminal */
  count: number;
  template: SlotSpec;
  individual: boolean;
  slots: SlotSpec[];
  /** livre: pedido inicial, enviado a todos os terminais de IA (ou só aos que o editam em "Configurar cada terminal") */
  prompt: string;
  sendToAll: boolean;
  /** squad */
  squadId: string | null;
  brief: string;
  answers: Record<number, string>;
  /** "Já tenho tudo definido": esconde as perguntas do intake */
  intakeHidden: boolean;
  /** agêntico */
  pilot: Pilot;
  style: "chain" | "torre" | "plano" | "paralelo";
  /** perfil de trabalho da missão (id); null = do projeto/padrão */
  profile: string | null;
}

export function initialWizard(template: SlotSpec, style: "chain" | "torre" | "plano" | "paralelo"): WizardState {
  return {
    mode: "livre", recipeId: null, name: "", worktree: false,
    count: 2, template, individual: false, slots: [], prompt: "", sendToAll: true,
    squadId: null, brief: "", answers: {}, intakeHidden: false,
    pilot: { cli: null, model: null, effort: null, account: null }, style, profile: null,
  };
}

/** Respostas que o brief já dá: a primeira URL e a referência após "inspirado em/no/na…". Mesmas regras de squads::autofill. */
const URL_RE = /https?:\/\/[^\s<>"]+/;
const INSPIRED_RE = /inspirad[oa]\s+(?:em|no|na|pelo|pela)\s+([^\s,;!?]+)/i;

export function autofill(intake: IntakeQ[], brief: string): Record<number, string> {
  const out: Record<number, string> = {};
  intake.forEach((q, i) => {
    const found = q.autofill === "first_url" ? URL_RE.exec(brief)?.[0]
      : q.autofill === "inspired_by" ? INSPIRED_RE.exec(brief)?.[1]
      : undefined;
    const v = found?.replace(/\.+$/, "");
    if (v) out[i] = v;
  });
  return out;
}

/** Muda o brief: preenche as respostas vazias ou ainda iguais ao preenchimento anterior; o que o usuário editou fica. */
export function setBrief(state: WizardState, intake: IntakeQ[], brief: string): WizardState {
  const prev = autofill(intake, state.brief);
  const next = autofill(intake, brief);
  const answers = { ...state.answers };
  intake.forEach((_, i) => {
    const cur = answers[i];
    const wasAuto = cur === undefined || cur === "" || cur === prev[i];
    if (next[i] !== undefined && wasAuto) answers[i] = next[i];
    else if (next[i] === undefined && cur !== undefined && cur === prev[i]) delete answers[i];
  });
  return { ...state, brief, answers };
}

/** Nome e brief de uma issue do GitHub (P1.9). O nome tem até 80 caracteres. */
export function briefFromIssue(issue: GhIssue): { name: string; brief: string } {
  const title = issue.title.trim();
  const name = Array.from(`#${issue.number} ${title}`).slice(0, 80).join("");
  const labels = issue.labels.length ? `\nRótulos: ${issue.labels.join(", ")}` : "";
  const brief = `Issue #${issue.number}: ${title}${labels}\n\n${issue.body.trim()}`.trim();
  return { name, brief };
}

/** Escolher a squad: as respostas são as do intake dela (o que o brief já dá). */
export function pickSquad(state: WizardState, squad: SquadDef): WizardState {
  return { ...state, squadId: squad.id, answers: autofill(squad.intake, state.brief) };
}

/** Aplica a receita: modo, squad, piloto, worktree, estilo e respostas (ou os terminais, no modo livre). */
export function applyRecipe(recipe: RecipeDef, state: WizardState, squad?: SquadDef): WizardState {
  const next: WizardState = { ...state, recipeId: recipe.id };
  if (recipe.mode === "livre" || recipe.mode === "squad" || recipe.mode === "agentico") next.mode = recipe.mode;
  if (recipe.worktree !== null) next.worktree = recipe.worktree;
  if (recipe.style === "chain" || recipe.style === "torre" || recipe.style === "plano") next.style = recipe.style;
  if (recipe.pilot) next.pilot = { cli: recipe.pilot.cli ?? null, model: recipe.pilot.model ?? null, effort: recipe.pilot.effort ?? null, account: null };
  if (recipe.squad) {
    next.squadId = recipe.squad;
    if (squad && squad.id === recipe.squad) {
      const answers = autofill(squad.intake, state.brief);
      squad.intake.forEach((q, i) => {
        const a = recipe.intake[q.q];
        if (a !== undefined) answers[i] = a;
      });
      next.answers = answers;
    }
  }
  if (recipe.slots.length) {
    next.count = Math.min(64, Math.max(1, recipe.slots.length));
    next.slots = recipe.slots as SlotSpec[];
    next.individual = true;
  }
  return next;
}

/** Pedido inicial de um terminal: o texto editado nele (em "Configurar cada terminal"), senão o compartilhado se marcado. */
export function promptOf(state: WizardState, i: number): string | null {
  const own = state.individual ? state.slots[i]?.prompt : undefined;
  const text = own ?? (state.sendToAll ? state.prompt : "");
  return text?.trim() || null;
}

/** Os terminais do modo livre, um por posição. Terminal de IA leva o pedido inicial (`prompt`), se houver. */
export function chosenSlots(state: WizardState): SlotSpec[] {
  return Array.from({ length: state.count }, (_, i) => {
    const slot = state.individual ? state.slots[i] ?? state.template : state.template;
    const prompt = slot.kind === "agent" ? promptOf(state, i) : null;
    return prompt ? { ...slot, prompt } : slot;
  });
}

/** O que falta para iniciar (vazio = pode iniciar). */
export function missingFor(state: WizardState): string | null {
  if (state.mode === "squad") {
    if (!state.squadId) return "escolha uma squad";
    if (!state.brief.trim()) return "escreva o pedido da missão";
  }
  if (state.mode === "agentico" && !state.brief.trim()) return "escreva o pedido da missão";
  return null;
}

/** Monta o corpo de `mission_create`. Respostas vazias não entram (não apagam o preenchimento automático). */
export function missionInput(state: WizardState, ctx: { workspaceId: string; cwd: string }): NewMission {
  const base = { workspaceId: ctx.workspaceId, name: state.name.trim(), worktree: state.worktree, profile: state.profile ?? undefined };
  if (state.mode === "livre") {
    return { ...base, mode: "livre", slots: chosenSlots(state).map((s) => ({ ...s, workspaceId: ctx.workspaceId, cwd: ctx.cwd })) };
  }
  const recipe = state.recipeId ? { recipeId: state.recipeId } : {};
  if (state.mode === "squad") {
    const answers: Record<number, string> = {};
    for (const [k, v] of Object.entries(state.answers)) if (v.trim()) answers[Number(k)] = v.trim();
    return { ...base, ...recipe, mode: "squad", squadId: state.squadId ?? undefined, brief: state.brief, answers };
  }
  return { ...base, ...recipe, mode: "agentico", brief: state.brief, pilot: state.pilot, style: state.style };
}

/** O que o assistente lembra da missão: modo, nº de terminais, terminal padrão (sem o pedido) e worktree. */
export function memoryOf(state: WizardState): MissionMemory {
  return { mode: state.mode, count: state.count, worktree: state.worktree, template: { ...state.template, prompt: null } as Record<string, unknown> };
}

/** Aplica a memória da última missão ao assistente novo; sem memória, o estado não muda. */
export function withMemory(state: WizardState, m: MissionMemory | null, installed?: (cli: string) => boolean): WizardState {
  if (!m) return state;
  const template = { ...(m.template as Partial<SlotSpec>) };
  // a IA lembrada só vale se ainda estiver instalada; senão fica a padrão (state.template)
  if (installed && typeof template.cli === "string" && !installed(template.cli)) delete template.cli;
  return { ...state, mode: m.mode, count: m.count, worktree: m.worktree, template: { ...state.template, ...template } };
}

/** Predefinição "Comparar 3 modelos": 3 terminais com o mesmo terminal padrão, para trocar o modelo de cada um. */
export function presetCompare(state: WizardState): WizardState {
  return { ...state, mode: "livre", count: 3, individual: true, slots: [0, 1, 2].map(() => ({ ...state.template })) };
}

/** Predefinição "Builder + reviewer": 2 terminais; o primeiro constrói, o segundo revisa. */
export function presetBuilderReviewer(state: WizardState): WizardState {
  return { ...state, mode: "livre", count: 2, individual: true, slots: [{ ...state.template, role: "builder" }, { ...state.template, role: "reviewer" }] };
}
