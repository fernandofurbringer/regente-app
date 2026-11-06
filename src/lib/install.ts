import type { PaneSpec } from "./ipc";

/** Comando com sintaxe do PowerShell (`irm`, `iex`, pipe): roda no PowerShell, qualquer que seja o shell padrão. */
export const needsPowerShell = (cmd: string): boolean => /\b(irm|iex)\b|\|/.test(cmd);

/** Spec do painel de instalação de uma IA. */
export function installSpec(label: string, cmd: string): Omit<PaneSpec, "workspaceId" | "cwd"> {
  return { kind: "terminal", name: `Instalar ${label}`, run: cmd, shell: needsPowerShell(cmd) ? "powershell" : undefined };
}
