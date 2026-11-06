import type { Detected, NewMission, PaneSpec, Workspace } from "./ipc";

/**
 * Workspace para um card que precisa de pasta: o ativo, ou a pasta escolhida agora (ela abre como aba e
 * segue o card). `null` = o usuário cancelou o seletor; nada acontece.
 */
export async function ensureWorkspace(
  current: Workspace | null,
  pickFolder: () => Promise<string | null>,
  openWorkspace: (path: string) => Promise<Workspace | null>,
): Promise<Workspace | null> {
  if (current) return current;
  const dir = await pickFolder();
  return dir ? openWorkspace(dir) : null;
}

/** Cards de início rápido (primeira execução e Home vazia). Os ids são gravados em `quickstartDismissed`. */
export interface QuickCard { id: string; title: string; text: string; action: string }

export const QUICK_CARDS: QuickCard[] = [
  { id: "pasta", title: "Abrir uma pasta", text: "Escolha a pasta do projeto. Ela vira uma aba.", action: "Escolher pasta" },
  { id: "primeira", title: "Primeira missão (Livre, 2 {ia})", text: "Dois terminais com {ia}, lado a lado, no workspace atual.", action: "Criar missão" },
  { id: "squad", title: "Squad de exemplo", text: "Abre o assistente de missão com um time pronto para começar.", action: "Ver squads" },
  { id: "voz", title: "Configurar voz", text: "Ditado no terminal e voz local.", action: "Abrir voz" },
  { id: "celular", title: "Parear o celular", text: "Controle os painéis pelo celular, na mesma rede.", action: "Abrir celular" },
  { id: "atalhos", title: "Ver atalhos (Ctrl+Shift+P)", text: "A paleta de comandos encontra tudo pelo teclado, de qualquer lugar.", action: "Ver atalhos" },
];

/** Cards que o usuário ainda não fechou, na ordem da lista. `{ia}` nos textos vira o nome da IA padrão. */
export function visibleCards(dismissed: string[], ia = "Claude Code"): QuickCard[] {
  return QUICK_CARDS.filter((c) => !dismissed.includes(c.id)).map((c) => ({ ...c, title: c.title.split("{ia}").join(ia), text: c.text.split("{ia}").join(ia) }));
}

const CLI_ORDER = ["claude", "codex", "gemini"];

/** Pura: a IA padrão é a primeira de claude > codex > gemini que está instalada; null se nenhuma. */
export function defaultCli(detected: Detected[]): string | null {
  const installed = (id: string) => detected.some((d) => d.kind === "cli" && d.id === id && d.path);
  return CLI_ORDER.find(installed) ?? null;
}

/** Os dois terminais da primeira missão (modo livre, permissão de pedir confirmação). */
export function firstMissionInput(workspaceId: string, cwd: string, cli = "claude"): NewMission {
  const slot: PaneSpec = { workspaceId, cwd, kind: "agent", cli, permission: "pend" };
  return { workspaceId, name: "Primeira missão", mode: "livre", worktree: false, slots: [slot, { ...slot }] };
}

/** IA do painel padrão: a última escolhida, se ainda estiver instalada; senão a primeira instalada (`defaultCli`). */
export function defaultAgentCli(detected: Detected[], last: string | null): string | null {
  const installed = (id: string) => detected.some((d) => d.kind === "cli" && d.id === id && d.path);
  return last && installed(last) ? last : defaultCli(detected);
}

/** Id e nome da IA do painel padrão (o nome aparece nos cards e nos botões). */
export function defaultAgentLabel(detected: Detected[], last: string | null, clis: { id: string; label: string }[]): { cli: string; label: string } {
  const cli = defaultAgentCli(detected, last) ?? "claude";
  return { cli, label: clis.find((c) => c.id === cli)?.label ?? cli };
}
