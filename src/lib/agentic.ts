import type { Decision, TaskType } from "./ipc";

/** Endereço do Decisor: https, ou http só para o próprio computador (a chave não viaja em texto claro). Vazio = erro de texto. */
export function baseUrlError(url: string, kind: string): string | null {
  const u = url.trim();
  if (!u) return kind === "openai" ? "informe a URL do provedor" : null;
  let parsed: URL;
  try {
    parsed = new URL(u);
  } catch {
    return "URL inválida";
  }
  if (parsed.protocol === "https:") return null;
  const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(parsed.hostname);
  if (parsed.protocol === "http:" && loopback) return null;
  return "a URL do Decisor precisa ser https (http só em localhost)";
}

/** Id de um tipo novo a partir do nome: minúsculas e hífens, sem colidir com os ids existentes. */
export function slugId(name: string, taken: string[]): string {
  const base = name.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "tipo";
  let out = base;
  for (let n = 2; taken.includes(out); n++) out = `${base}-${n}`;
  return out;
}

/** Textarea de uma lista: um item por linha. */
export const linesToList = (text: string) => text.split("\n").map((s) => s.trim()).filter(Boolean);
export const listToLines = (items: string[]) => items.join("\n");

export interface Spend { count: number; costUsd: number; errors: number }

/** Decisões a partir de `since`: quantas, quanto custaram e quantas falharam (resposta inválida do Decisor, sem tipo). */
export function spend(decisions: Decision[], since: number): Spend {
  let count = 0, costUsd = 0, errors = 0;
  for (const d of decisions) {
    if (d.createdAt < since) continue;
    count++;
    costUsd += d.costUsd;
    if (d.taskType === null) errors++;
  }
  return { count, costUsd, errors };
}

/** Gasto por tipo de tarefa a partir de `since`. Decisões sem tipo ficam em "(sem tipo)". */
export function spendByType(decisions: Decision[], since: number): Record<string, Spend> {
  const out: Record<string, Spend> = {};
  for (const d of decisions) {
    if (d.createdAt < since) continue;
    const key = d.taskType ?? "(sem tipo)";
    const cur = out[key] ?? { count: 0, costUsd: 0, errors: 0 };
    cur.count++;
    cur.costUsd += d.costUsd;
    if (d.taskType === null) cur.errors++;
    out[key] = cur;
  }
  return out;
}

/** Texto do pedido cortado em 80 caracteres. */
export const short = (text: string, max = 80) => (text.length > max ? `${text.slice(0, max - 1)}…` : text);

/** Nome do tipo para mostrar (o id quando o tipo já não existe). */
export const typeName = (types: TaskType[], id: string | null) => (id === null ? "(sem tipo)" : types.find((t) => t.id === id)?.name ?? id);
