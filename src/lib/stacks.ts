import type { NewMission, PaneRow, PaneSpec, RecipeDef } from "./ipc";

/** Stacks são receitas leves de modo livre, com id `stack-<carimbo>`. */
export const STACK_PREFIX = "stack-";
export const isStack = (r: RecipeDef): boolean => r.mode === "livre" && r.id.startsWith(STACK_PREFIX);

/**
 * Receita com os painéis de IA abertos (CLI, modelo, esforço, permissão, nome e conta). Sem prompt.
 * `danger` vira `pend`, porque a receita recusa essa permissão. Só o nome dado à mão é salvo: o automático
 * fica de fora, para a stack também nomear pelo primeiro pedido.
 */
export function stackFromPanes(panes: PaneRow[], name: string, now: number): RecipeDef {
  const slots = panes
    .filter((p) => p.kind === "agent" && !!p.cli && p.closedAt === null)
    .map((p) => ({
      cli: p.cli ?? undefined,
      model: p.model ?? undefined,
      effort: p.effort ?? undefined,
      permission: p.permission === "danger" ? "pend" : p.permission,
      name: p.nameManual ? p.name : undefined,
      account: p.account ?? undefined,
    }));
  return { id: `${STACK_PREFIX}${now}`, name, description: `${slots.length} painéis`, mode: "livre", squad: null, pilot: null, slots, intake: {}, worktree: null, style: null, source: "regente" };
}

/** Corpo de `mission_create` que abre todos os painéis da stack no workspace. */
export function stackMissionInput(r: RecipeDef, ws: { id: string; path: string }): NewMission {
  const slots = r.slots.map((s) => ({ ...s, kind: "agent", workspaceId: ws.id, cwd: ws.path }) as unknown as PaneSpec);
  return { workspaceId: ws.id, name: r.name, mode: "livre", worktree: false, slots };
}

type Slot = Omit<PaneSpec, "workspaceId" | "cwd">;

/** Stack de um painel só: a que o Catálogo edita e o "Novo painel" aplica. */
export const isSingleStack = (r: RecipeDef): boolean => isStack(r) && r.slots.length === 1;

/** Campos que o Catálogo edita numa stack (agente + skill + modelo + esforço + permissão). */
export interface StackForm { name: string; cli: string; model: string; effort: string; agent: string; skill: string; permission: "plan" | "pend" | "auto" }

/** Receita de um slot só. Vazio = o padrão do CLI/agente. A skill escolhida substitui as do agente. */
export function stackFromForm(f: StackForm, id: string): RecipeDef {
  const slot: Record<string, unknown> = { cli: f.cli, permission: f.permission };
  if (f.model.trim()) slot.model = f.model.trim();
  if (f.effort) slot.effort = f.effort;
  if (f.agent) slot.agent = f.agent;
  if (f.skill) slot.allowedSkills = [f.skill];
  const description = [f.agent, f.skill, f.model.trim(), f.effort].filter(Boolean).join(" · ");
  return { id, name: f.name.trim(), description, mode: "livre", squad: null, pilot: null, slots: [slot], intake: {}, worktree: null, style: null, source: "regente" };
}

// ponytail: só a primeira skill do slot aparece no formulário; uma stack editada à mão com várias volta com uma só ao salvar
export function formFromStack(r: RecipeDef): StackForm {
  const s = r.slots[0] ?? {};
  const str = (k: string): string => (typeof s[k] === "string" ? (s[k] as string) : "");
  const skills = Array.isArray(s.allowedSkills) ? (s.allowedSkills as unknown[]) : [];
  const perm = str("permission");
  return {
    name: r.name,
    cli: str("cli") || "claude",
    model: str("model"),
    effort: str("effort"),
    agent: str("agent"),
    skill: typeof skills[0] === "string" ? skills[0] : "",
    permission: perm === "plan" || perm === "auto" ? perm : "pend",
  };
}

/** O slot que o "Novo painel" preenche ao escolher a stack (sem nome nem pedido: esses o usuário digita). */
export function slotFromStack(r: RecipeDef): Slot | null {
  if (r.slots.length !== 1) return null;
  const f = formFromStack(r);
  return { kind: "agent", cli: f.cli, permission: f.permission, model: f.model || null, effort: f.effort || null, agent: f.agent || null, allowedSkills: f.skill ? [f.skill] : null };
}
