import { colorFor, countdown, resetText, type RPayload, type RWindow } from "./redline";

/** Um chip do cabeçalho: uma janela de uso de uma conta do Claude marcada para o topo. */
export interface UsageChip {
  /** `provider/account/window` (chave do React) */
  key: string;
  /** rótulo curto: 5h, 7d, 7d sonnet… */
  label: string;
  /** percentual inteiro, 0 a 100 */
  pct: number;
  /** cor do Redline (verde < 60, âmbar < 85, vermelho) */
  color: string;
  /** tempo até reiniciar (2h10) ou null quando a janela não informa */
  reset: string | null;
  /** a última consulta dessa conta falhou: a leitura é antiga */
  stale: boolean;
  /** nome da conta, só quando há mais de uma conta do Claude no topo */
  account: string | null;
  /** dica completa do chip */
  title: string;
}

const ORDER = ["five_hour", "seven_day"];

/** five_hour → 5h, seven_day → 7d, seven_day_<modelo> → 7d <modelo>; o resto como vem. */
export function chipLabel(window: string): string {
  if (window === "five_hour") return "5h";
  if (window === "seven_day") return "7d";
  if (window.startsWith("seven_day_")) return `7d ${window.slice("seven_day_".length)}`;
  return window;
}

const rank = (w: RWindow): number => {
  const i = ORDER.indexOf(w.window);
  return i === -1 ? ORDER.length : i;
};

/** Chips do cabeçalho: as janelas do Claude das contas em `topo`, por conta; 5h antes de 7d, depois as demais por nome. */
export function usageChips(p: RPayload | null, now: number): UsageChip[] {
  if (!p) return [];
  const keys = new Set(p.topo);
  const many = p.topo.filter((k) => k.startsWith("claude/")).length > 1;
  const wins = p.windows.filter((w) => w.provider === "claude" && keys.has(`${w.provider}/${w.account}`));
  wins.sort((a, b) => a.account.localeCompare(b.account) || rank(a) - rank(b) || a.window.localeCompare(b.window));
  return wins.map((w) => {
    const stale = p.stale.some((s) => s.provider === w.provider && s.account === w.account);
    // o número mostrado e a cor vêm do mesmo valor arredondado (59,6 → 60 % âmbar), limitado a 0..100
    const pct = Math.min(100, Math.max(0, Math.round(w.usedPercent)));
    const reset = w.resetsAt === null ? null : countdown(w.resetsAt - now);
    const label = chipLabel(w.window);
    const title = [`Claude · ${w.account} · ${label}: ${pct}%`, resetText(w.resetsAt, now), stale && "leitura antiga: a última consulta falhou"]
      .filter(Boolean).join(" · ");
    return { key: `${w.provider}/${w.account}/${w.window}`, label, pct, color: colorFor(pct), reset, stale, account: many ? w.account : null, title };
  });
}
