/** Frequências da tela de agendamento; a expressão cron é gerada daqui (numeração padrão: 0 e 7 = domingo, 1 = segunda). */
export type Freq = "daily" | "weekly" | "monthly" | "custom";

export const WEEKDAYS = [
  { n: 1, label: "Seg" }, { n: 2, label: "Ter" }, { n: 3, label: "Qua" }, { n: 4, label: "Qui" }, { n: 5, label: "Sex" }, { n: 6, label: "Sáb" }, { n: 0, label: "Dom" },
];

/** Atalhos: a expressão aparece na tela para quem quiser ajustar. */
export const PRESETS: { label: string; expr: string }[] = [
  { label: "Todo dia às 9h", expr: "0 9 * * *" },
  { label: "Dias úteis às 9h", expr: "0 9 * * 1-5" },
  { label: "Toda segunda às 9h", expr: "0 9 * * 1" },
  { label: "Todo dia 1 às 9h", expr: "0 9 1 * *" },
  { label: "A cada minuto (teste)", expr: "* * * * *" },
];

/** "09:30" -> { h: 9, m: 30 }; null se inválido. */
export function parseTime(t: string): { h: number; m: number } | null {
  const r = /^(\d{1,2}):(\d{2})$/.exec(t.trim());
  if (!r) return null;
  const h = Number(r[1]), m = Number(r[2]);
  return h <= 23 && m <= 59 ? { h, m } : null;
}

/** Expressão de 5 campos a partir da frequência. Lança erro com texto em pt-BR quando falta algo. */
export function buildExpr(freq: Exclude<Freq, "custom">, time: string, days: number[], monthDay: number): string {
  const t = parseTime(time);
  if (!t) throw new Error("hora inválida (use HH:MM)");
  if (freq === "daily") return `${t.m} ${t.h} * * *`;
  if (freq === "weekly") {
    if (!days.length) throw new Error("escolha pelo menos um dia da semana");
    return `${t.m} ${t.h} * * ${[...new Set(days)].sort((a, b) => a - b).join(",")}`;
  }
  if (!Number.isInteger(monthDay) || monthDay < 1 || monthDay > 31) throw new Error("dia do mês: 1 a 31");
  return `${t.m} ${t.h} ${monthDay} * *`;
}

/** Reconhece a frequência de uma expressão já salva (para o formulário de edição). Outras = personalizado. */
export function parseExpr(expr: string): { freq: Freq; time: string; days: number[]; monthDay: number } {
  const f = expr.trim().split(/\s+/);
  const fallback = { freq: "custom" as Freq, time: "09:00", days: [1], monthDay: 1 };
  if (f.length !== 5) return fallback;
  const [m, h, dom, mon, dow] = f;
  if (!/^\d+$/.test(m) || !/^\d+$/.test(h) || Number(m) > 59 || Number(h) > 23) return fallback;
  const time = `${h.padStart(2, "0")}:${m.padStart(2, "0")}`;
  if (dom === "*" && mon === "*" && dow === "*") return { ...fallback, freq: "daily", time };
  if (dom === "*" && mon === "*" && /^[0-7](,[0-7])*$/.test(dow)) {
    return { ...fallback, freq: "weekly", time, days: [...new Set(dow.split(",").map((d) => Number(d) % 7))] };
  }
  if (/^\d+$/.test(dom) && mon === "*" && dow === "*" && Number(dom) >= 1 && Number(dom) <= 31) {
    return { ...fallback, freq: "monthly", time, monthDay: Number(dom) };
  }
  return fallback;
}

/** Data/hora curta em pt-BR de um instante em ms; "—" quando não há. */
export const fmtWhen = (ms: number | null) => (ms === null ? "—" : new Date(ms).toLocaleString("pt-BR", { dateStyle: "short", timeStyle: "short" }));

/** Texto de um dia da semana em pt-BR a partir do número padrão. */
export const weekdayLabel = (n: number) => WEEKDAYS.find((w) => w.n === n % 7)?.label ?? "?";
