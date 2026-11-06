import { useEffect, useMemo, useState } from "react";
import { api, errMsg, on } from "../lib/ipc";
import { colorFor, countdown, labelOf, peakHour, seriesOf, type RPayload, type RPoint } from "../lib/redline";
import { useStore } from "../state/store";

const PALETTE = ["#58a6ff", "#3fb950", "#d29922", "#f85149", "#bc8cff", "#39c5cf"];
const W = 640;
const H = 160;
const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const agoText = (ms: number, now: number) => `há ${Math.max(1, Math.round((now - ms) / 60_000))} min`;

/** One line per window: x from the window's start to now, y from 0 to 100%. */
function Chart({ points, from, now, days }: { points: RPoint[]; from: number; now: number; days: 7 | 30 }) {
  const series = [...seriesOf(points).entries()];
  if (!series.length) return <div className="meta">Sem histórico ainda.</div>;
  const x = (ts: number) => ((ts - from) / Math.max(1, now - from)) * W;
  const y = (pct: number) => H - (Math.min(100, Math.max(0, pct)) / 100) * H;
  return (
    <div>
      <svg viewBox={`0 0 ${W} ${H}`} width="100%" role="img" aria-label="Uso ao longo do tempo" style={{ background: "var(--bg3)", borderRadius: 6 }}>
        {[25, 50, 75].map((g) => <line key={g} x1={0} x2={W} y1={y(g)} y2={y(g)} stroke="var(--line)" strokeDasharray="4 4" />)}
        {series.map(([k, pts], i) =>
          pts.length === 1 ? (
            <circle key={k} cx={x(pts[0].ts)} cy={y(pts[0].usedPercent)} r={3} fill={PALETTE[i % PALETTE.length]} />
          ) : (
            <polyline key={k} fill="none" stroke={PALETTE[i % PALETTE.length]} strokeWidth={2}
              points={pts.map((p) => `${x(p.ts).toFixed(1)},${y(p.usedPercent).toFixed(1)}`).join(" ")} />
          ),
        )}
      </svg>
      <div className="row" style={{ gap: 10, flexWrap: "wrap", marginTop: 4 }}>
        {series.map(([k], i) => (
          <span key={k} className="meta"><span style={{ color: PALETTE[i % PALETTE.length] }}>■</span> {k}</span>
        ))}
        <span className="meta" style={{ marginLeft: "auto" }}>uso em % · {days === 7 ? "7" : "30"} dias</span>
      </div>
    </div>
  );
}

/** Redline: barras atuais por conta, gráficos de 7 e 30 dias e o horário de maior consumo. */
export default function RedlineWindow() {
  const [data, setData] = useState<RPayload | null>(null);
  const [points, setPoints] = useState<RPoint[]>([]);
  const [days, setDays] = useState<7 | 30>(7);
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.redlineCurrent().then(setData).catch(toast);
    const sub = on<RPayload>("limits", setData);
    const tick = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      window.clearInterval(tick);
      sub.then((u) => u()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    api.redlineHistory(days).then(setPoints).catch(toast);
  }, [days, data]);

  async function refresh() {
    setBusy(true);
    try {
      setData(await api.redlineNow());
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  // one card per provider/account
  const cards = useMemo(() => {
    const m = new Map<string, RPayload["windows"]>();
    for (const w of data?.windows ?? []) {
      const k = `${w.provider}/${w.account}`;
      m.set(k, [...(m.get(k) ?? []), w]);
    }
    return [...m.entries()];
  }, [data]);
  const peak = useMemo(() => peakHour(points), [points]);

  return (
    <div style={{ padding: 16, display: "flex", flexDirection: "column", gap: 14, height: "100vh", boxSizing: "border-box", overflow: "auto" }}>
      <div className="row" style={{ gap: 8 }}>
        <h2 style={{ margin: 0 }}>Limites de uso</h2>
        <span className="meta">lido a cada 5 min · somente leitura</span>
        <span style={{ flex: 1 }} />
        <button className="btn" disabled={busy} onClick={refresh}>{busy ? "Atualizando…" : "Atualizar"}</button>
      </div>

      {!data || cards.length === 0 ? (
        <div className="meta">Nenhuma conta com leitura ainda. Ative contas em Configurações → Limites de uso.</div>
      ) : (
        cards.map(([key, ws]) => {
          const stale = data.stale.find((s) => `${s.provider}/${s.account}` === key);
          return (
            <section key={key} className="card" style={{ border: "1px solid var(--line)", borderRadius: 8, padding: 12, background: "var(--bg2)" }}>
              <div className="row" style={{ gap: 8 }}>
                <strong>{key.startsWith("claude") ? "Claude" : "Codex"} · {key.split("/")[1]}</strong>
                {stale && <span className="kbd" title="A última consulta falhou; mostrando a última boa">{agoText(stale.at, now)}</span>}
              </div>
              {ws.map((w) => (
                <div key={w.window} style={{ marginTop: 8 }}>
                  <div className="row" style={{ justifyContent: "space-between" }}>
                    <span>{labelOf(w.window)}</span>
                    <span>
                      {Math.round(w.usedPercent)}%{w.resetsAt !== null && <span className="meta"> · reseta em {countdown(w.resetsAt - now)}</span>}
                    </span>
                  </div>
                  <div role="progressbar" aria-valuenow={Math.round(w.usedPercent)} aria-valuemin={0} aria-valuemax={100} style={{ height: 8, background: "var(--bg4)", borderRadius: 4, overflow: "hidden" }}>
                    <div style={{ width: `${Math.min(100, w.usedPercent)}%`, height: "100%", background: colorFor(w.usedPercent) }} />
                  </div>
                </div>
              ))}
            </section>
          );
        })
      )}

      <section>
        <div className="row" style={{ gap: 8 }}>
          <strong>Histórico</strong>
          <span style={{ flex: 1 }} />
          <button className={`tb-btn ${days === 7 ? "on" : ""}`} aria-pressed={days === 7} onClick={() => setDays(7)}>7 dias</button>
          <button className={`tb-btn ${days === 30 ? "on" : ""}`} aria-pressed={days === 30} onClick={() => setDays(30)}>30 dias</button>
        </div>
        <Chart points={points} from={now - days * 86_400_000} now={now} days={days} />
        <div className="meta" style={{ marginTop: 6 }}>
          Horário de maior consumo: {peak === null ? "sem aumentos registrados ainda" : `${String(peak).padStart(2, "0")}h`}
        </div>
      </section>
    </div>
  );
}
