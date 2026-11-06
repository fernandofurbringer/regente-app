import type { PaneRow } from "./ipc";

/** Painel Claude ocioso: o único tipo que "Hibernar ociosos" tenta (o servidor ainda confere os motivos). */
export const isIdleClaude = (p: Pick<PaneRow, "kind" | "cli" | "status" | "closedAt">) =>
  p.kind === "agent" && p.cli === "claude" && p.status === "ocioso" && p.closedAt === null;

/** Memória em pt-BR: "870 MB", "3,1 GB". */
export function fmtMem(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toLocaleString("pt-BR", { maximumFractionDigits: 1 })} GB` : `${Math.round(mb)} MB`;
}
