import type { UsageRow } from "./ipc";

/** 1 casa abaixo de 10 (1.5k), inteiro acima; arredonda antes de escolher a unidade (9 999 → 10k, 999 999 → 1M). */
const short = (v: number) => String(v < 9.95 ? Number(v.toFixed(1)) : Math.round(v));

export function fmtTokens(n: number): string {
  if (n < 1000) return String(n);
  return Math.round(n / 1e3) < 1000 ? `${short(n / 1e3)}k` : `${short(n / 1e6)}M`;
}

/** Gasto abaixo de US$ 0,01 não vira "$0.00": mostra "<US$ 0,01" (mesmo texto do backend, budget::usd). */
export function fmtUsd(v: number): string {
  if (v > 0 && v < 0.01) return "<US$ 0,01";
  return `$${v.toFixed(2)}`;
}

/** Custo em pt-BR para a barra: "≈US$ 3,20" (≈ só com valor), abaixo de US$ 0,01 vira "<US$ 0,01". */
export function fmtUsdPt(v: number): string {
  if (v > 0 && v < 0.01) return "<US$ 0,01";
  return `${v > 0 ? "≈" : ""}US$ ${v.toFixed(2).replace(".", ",")}`;
}

/** Uso do orçamento diário: verde abaixo de 80%, âmbar de 80%, vermelho de 100%. */
export function budgetBand(spent: number, limit: number): { pct: number; band: "ok" | "warn" | "over" } {
  const pct = limit > 0 ? (spent / limit) * 100 : 0;
  return { pct, band: pct >= 100 ? "over" : pct >= 80 ? "warn" : "ok" };
}

export type Period = "hoje" | "7" | "30" | "tudo";

const ymd = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

export function sinceDay(p: Period, now = new Date()): string | null {
  if (p === "tudo") return null;
  const d = new Date(now);
  d.setDate(d.getDate() - (p === "hoje" ? 0 : Number(p) - 1));
  return ymd(d);
}

/** Todos os dias de `from` até `to` (AAAA-MM-DD, inclusive). */
export function dayRange(from: string, to: string): string[] {
  const out: string[] = [];
  const d = new Date(`${from}T00:00:00`);
  for (let k = ymd(d); k <= to; d.setDate(d.getDate() + 1), k = ymd(d)) out.push(k);
  return out;
}

export interface Agg { key: string; tokIn: number; tokOut: number; costUsd: number; terminals: number }

export function aggregate(rows: UsageRow[], by: "cli" | "model" | "effort" | "day"): Agg[] {
  const m = new Map<string, Agg>();
  for (const r of rows) {
    const k = r[by];
    const a = m.get(k) ?? { key: k, tokIn: 0, tokOut: 0, costUsd: 0, terminals: 0 };
    a.tokIn += r.tokIn;
    a.tokOut += r.tokOut;
    a.costUsd += r.costUsd;
    a.terminals += r.terminalsOpened;
    m.set(k, a);
  }
  return [...m.values()];
}
