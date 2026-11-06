import type { GitBranches, GitEntry } from "./ipc";

/** Nome do estado de um arquivo no commit. */
export const GIT_CODE: Record<GitEntry["code"], string> = { A: "novo", M: "alterado", D: "apagado", "?": "não rastreado" };

/** Base padrão do PR quando a missão não tem uma (workspace sem branch base escolhida). */
export const DEFAULT_BASE = "main";

/** Evento da janela depois de trocar de branch, puxar ou sincronizar: o rodapé relê o estado git. */
export const GIT_CHANGED = "regente:git";

/** O evento `GIT_CHANGED` de um workspace (só a aba dele relê). */
export const gitChanged = (wsId: string): Event => new CustomEvent(GIT_CHANGED, { detail: { wsId } });

/** O evento vale para `wsId`: sem workspace no evento (quem dispara não sabe), vale para todos. */
export function gitChangedOf(e: Event, wsId: string): boolean {
  const alvo = (e as CustomEvent<{ wsId?: string } | null>).detail?.wsId;
  return !alvo || alvo === wsId;
}

/** Dá para commitar: mensagem com texto e ao menos um arquivo marcado. */
export function canCommit(message: string, picked: string[]): boolean {
  return message.trim().length > 0 && picked.length > 0;
}

/** Aviso sobre de onde veio a sugestão de mensagem. */
export function sourceNote(source: "ia" | "modelo", aiError: string | null): string {
  if (source === "ia") return "Sugerida pela IA: confira antes de commitar.";
  return aiError ? `Sugerida pelo modelo, porque a IA falhou (${aiError}).` : "Sugerida pelo modelo: sem chave de IA no cofre.";
}

export interface BranchOption { name: string; kind: "local" | "remota" | "nova" }

/** Opções do seletor de branch: locais e remotas que contêm o filtro; um nome digitado que não existe vira "nova". */
export function branchOptions(b: GitBranches, query: string): BranchOption[] {
  const q = query.trim();
  const has = (n: string) => n.toLowerCase().includes(q.toLowerCase());
  const out: BranchOption[] = [
    ...b.local.filter(has).map((name) => ({ name, kind: "local" as const })),
    ...b.remote.filter(has).map((name) => ({ name, kind: "remota" as const })),
  ];
  const exists = b.local.includes(q) || b.remote.some((r) => r === q || r.slice(r.indexOf("/") + 1) === q);
  if (q && !/\s/.test(q) && !exists) out.push({ name: q, kind: "nova" });
  return out;
}

/** Rótulo do botão de sincronizar: "Publicar" sem remoto; senão o que entra (↓) e sai (↑) pela última busca. */
export function syncLabel(b: Pick<GitBranches, "upstream" | "ahead" | "behind">): string {
  if (!b.upstream) return "Publicar";
  const parts = [b.behind > 0 && `↓${b.behind}`, b.ahead > 0 && `↑${b.ahead}`].filter(Boolean);
  return parts.length ? `Sincronizar ${parts.join(" ")}` : "Sincronizar";
}
