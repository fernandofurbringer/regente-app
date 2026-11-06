//! Redline (F3): funções puras da janela e do chip. Textos e rótulos iguais aos do menu da bandeja.

export interface RWindow {
  provider: string;
  account: string;
  window: string;
  usedPercent: number;
  resetsAt: number | null;
}
export interface RPoint {
  ts: number;
  provider: string;
  account: string;
  window: string;
  usedPercent: number;
}
export interface RPayload {
  windows: RWindow[];
  stale: { provider: string; account: string; at: number }[];
  topo: string[];
}

/** five_hour → 5h, seven_day → semana; other keys as they are. */
export const labelOf = (window: string): string => (window === "five_hour" ? "5h" : window === "seven_day" ? "semana" : window);

/** Time left: agora, 45min, 2h10, 3d4h. */
export function countdown(msLeft: number): string {
  if (msLeft <= 0) return "agora";
  const mins = Math.floor(msLeft / 60_000);
  if (mins < 60) return `${mins}min`;
  const h = Math.floor(mins / 60);
  if (h < 24) return `${h}h${String(mins % 60).padStart(2, "0")}`;
  return `${Math.floor(h / 24)}d${h % 24}h`;
}

/** Bar color by use: green below 60%, amber below 85%, red from there. */
export function colorFor(pct: number): string {
  if (pct >= 85) return "#f85149";
  if (pct >= 60) return "#d29922";
  return "#3fb950";
}

/** Hour of the day (0–23) with the highest average increase of use. Increases count inside one window's series and
 *  belong to the hour of the later reading. Null when nothing went up. */
export function peakHour(points: RPoint[]): number | null {
  const groups = new Map<string, RPoint[]>();
  for (const p of points) {
    const k = `${p.provider}/${p.account}/${p.window}`;
    const g = groups.get(k) ?? [];
    g.push(p);
    groups.set(k, g);
  }
  const sum = new Array<number>(24).fill(0);
  const n = new Array<number>(24).fill(0);
  for (const g of groups.values()) {
    g.sort((a, b) => a.ts - b.ts);
    for (let i = 1; i < g.length; i++) {
      const d = g[i].usedPercent - g[i - 1].usedPercent;
      if (d > 0) {
        const h = new Date(g[i].ts).getHours();
        sum[h] += d;
        n[h] += 1;
      }
    }
  }
  let best: number | null = null;
  let bestAvg = 0;
  for (let h = 0; h < 24; h++) {
    if (n[h] && sum[h] / n[h] > bestAvg) {
      bestAvg = sum[h] / n[h];
      best = h;
    }
  }
  return best;
}

/** The top bar's number: the worst use among the accounts marked for it. Null when none is marked (or read). */
export function worstTopBar(p: RPayload): number | null {
  const pct = p.windows.filter((w) => p.topo.includes(`${w.provider}/${w.account}`)).map((w) => w.usedPercent);
  return pct.length ? Math.max(...pct) : null;
}

/** Pior uso de uma conta (todas as janelas) para o cabeçalho do painel. Null sem leitura. */
export function accountPeak(p: RPayload, provider: string | null, account: string): number | null {
  const pct = p.windows.filter((w) => w.provider === provider && w.account === account).map((w) => w.usedPercent);
  return pct.length ? Math.max(...pct) : null;
}

/** "reinicia em 2h10", "reinicia agora" quando a janela já passou, ou null quando a janela não informa o reset. */
export function resetText(resetsAt: number | null, now: number): string | null {
  if (resetsAt === null) return null;
  const left = countdown(resetsAt - now);
  return left === "agora" ? "reinicia agora" : `reinicia em ${left}`;
}

/** The history grouped into one series per provider/account/window, each oldest first (for the SVG lines). */
export function seriesOf(points: RPoint[]): Map<string, RPoint[]> {
  const out = new Map<string, RPoint[]>();
  for (const p of [...points].sort((a, b) => a.ts - b.ts)) {
    const k = `${p.provider}/${p.account}/${labelOf(p.window)}`;
    out.set(k, [...(out.get(k) ?? []), p]);
  }
  return out;
}
