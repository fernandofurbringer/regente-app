import { useEffect, useState } from "react";
import { aggregate, dayRange, fmtTokens, fmtUsd, sinceDay, type Period } from "../../lib/format";
import { api, errMsg, type EconomyReport, type QualityGuard, type Settings, type UsageRow, type WasteRow } from "../../lib/ipc";
import { useStore } from "../../state/store";
import { TIPO_TAREFA_LABEL } from "./ProfileSection";
import CodeIndexCard from "../CodeIndexCard";

const PERIODS: [Period, string][] = [["hoje", "Hoje"], ["7", "7 dias"], ["30", "30 dias"], ["tudo", "Tudo"]];

/** Nomes em português das linhas de gasto que não são uma IA de terminal. */
const CLI_LABEL: Record<string, string> = { decisor: "Decisor", jarvis: "IA auxiliar", stt: "Voz (transcrição)" };
const cliName = (k: string) => CLI_LABEL[k] ?? k;

/** Vazio ou inválido = sem limite. */
const parseUsd = (v: string): number | null => {
  const n = Number(v.trim().replace(",", "."));
  return v.trim() && Number.isFinite(n) && n > 0 ? n : null;
};

/** Orçamento diário e por missão: aviso em 80% e 100%, e opção de pausar a missão que estourou. */
function BudgetCard() {
  const budget = useStore((s) => s.settings?.budget);
  const dailyUsd = budget?.dailyUsd ?? null;
  const missionUsd = budget?.missionUsd ?? null;
  const pauseOnLimit = budget?.pauseOnLimit ?? false;
  const [daily, setDaily] = useState("");
  const [mission, setMission] = useState("");
  useEffect(() => {
    setDaily(dailyUsd == null ? "" : String(dailyUsd));
    setMission(missionUsd == null ? "" : String(missionUsd));
  }, [dailyUsd, missionUsd]);

  async function save(p: Partial<Settings["budget"]>) {
    const st = useStore.getState();
    if (!st.settings) return;
    try {
      await st.saveSettings({ ...st.settings, budget: { ...st.settings.budget, ...p } });
    } catch (e) {
      st.toast(errMsg(e), "erro");
    }
  }
  const commit = (value: string, field: "dailyUsd" | "missionUsd") => {
    const next = parseUsd(value);
    if (next !== (field === "dailyUsd" ? dailyUsd : missionUsd)) void save({ [field]: next });
  };

  return (
    <div className="card">
      <strong>Orçamento de custo</strong>
      <small style={{ display: "block", color: "var(--fg3)" }}>Avisa ao chegar a 80% e a 100% (toast e aviso do Windows). Deixe vazio para não limitar.</small>
      <label className="field"><span>Diário (US$)</span>
        <input inputMode="decimal" value={daily} onChange={(e) => setDaily(e.target.value)} onBlur={() => commit(daily, "dailyUsd")}
          onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
      </label>
      <label className="field"><span>Por missão (US$)</span>
        <input inputMode="decimal" value={mission} onChange={(e) => setMission(e.target.value)} onBlur={() => commit(mission, "missionUsd")}
          onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
      </label>
      <label className="row"><input type="checkbox" checked={pauseOnLimit} onChange={(e) => save({ pauseOnLimit: e.target.checked })} /> Ao atingir 100% da missão, pausar os novos painéis dela</label>
    </div>
  );
}

const WASTE_LABEL: Record<string, string> = {
  repeated_read: "Leitura repetida", huge_output: "Saída gigante", cache_churn: "Churn de cache",
  cold_cache_gap: "Cache frio depois de pausa", retry_loop: "Repetição de chamadas", bloated_context: "Contexto inchado",
};
/** Dica por tipo; o botão leva a Configurações → Perfil de trabalho. */
const WASTE_HINT: Record<string, string> = {
  repeated_read: "Ative o índice de código ou peça só o trecho.", huge_output: "Ative o resumo de saídas longas.",
  cache_churn: "Escolha o modelo ao abrir o painel.", cold_cache_gap: "Use cache de 1 hora nos painéis que ficam parados.",
  retry_loop: "Divida a tarefa ou use /clear.", bloated_context: "Compacte ou recomece com memória na fronteira da tarefa.",
};

/** Onde os tokens foram desperdiçados (RM-21). Estimativas locais; nada sai do computador. */
function WasteCard() {
  const [rows, setRows] = useState<WasteRow[] | null>(null);
  const [erro, setErro] = useState(false);
  useEffect(() => {
    let alive = true;
    api.wasteSummary(Date.now() - 7 * 86_400_000, null)
      .then((r) => { if (alive) setRows(r); })
      .catch(() => { if (alive) { setRows([]); setErro(true); } });
    return () => { alive = false; };
  }, []);
  return (
    <div className="card">
      <strong>Onde os tokens foram desperdiçados (últimos 7 dias)</strong>
      <small style={{ display: "block", color: "var(--fg3)" }}>Valores estimados (bytes ÷ 4) a partir das conversas deste computador.</small>
      {rows === null ? <div className="meta" aria-live="polite">Lendo as conversas…</div>
        : rows.length === 0 ? <div className="meta">{erro ? "Não consegui ler as conversas." : "Nada desperdiçado nos últimos 7 dias."}</div>
        : (
          <ul>
            {rows.map((r) => (
              <li key={r.kind} className="row" style={{ alignItems: "baseline" }}>
                <span>
                  <strong>{WASTE_LABEL[r.kind] ?? r.kind}</strong>: ~{fmtTokens(r.tokensEst)} tokens estimados em {r.panes} painel(is), {r.count} achado(s).
                  {r.removidos > 0 && <> <small style={{ color: "var(--fg3)" }}>(painel removido: {r.removidos})</small></>}
                  {r.exemplo && <> {r.exemplo}.</>} <small style={{ color: "var(--fg3)" }}>{WASTE_HINT[r.kind] ?? ""}</small>
                </span>
                <button className="btn" onClick={() => useStore.getState().patch({ settingsOpen: true, settingsSection: "perfil" })}>Aplicar sugestão</button>
              </li>
            ))}
          </ul>
        )}
    </div>
  );
}

const pct = (v: number | null) => (v == null ? "sem dados" : `${Math.round(v * 100)}%`);

/** Cache, contexto, economia estimada e qualidade (RM-23). Só leitura do banco. */
function EconomiaCards() {
  const active = useStore((s) => s.active);
  const [rep, setRep] = useState<EconomyReport | null>(null);
  const [qual, setQual] = useState<QualityGuard | null>(null);
  useEffect(() => {
    let alive = true;
    api.economyReport(Date.now() - 7 * 86_400_000, null).then((r) => { if (alive) setRep(r); }).catch(() => { if (alive) setRep(null); });
    return () => { alive = false; };
  }, []);
  useEffect(() => {
    let alive = true;
    setQual(null);
    if (active) api.qualityGuard(active).then((q) => { if (alive) setQual(q); }).catch(() => { if (alive) setQual(null); });
    return () => { alive = false; };
  }, [active]);
  return (
    <>
      <div className="card">
        <strong>Acerto de cache: {pct(rep?.acertoCache ?? null)}</strong> <small style={{ color: "var(--fg3)" }}>(meta ≥ 80%, últimos 7 dias)</small>
        <div><strong>Contexto médio por turno:</strong> {rep?.contextoMedioTurno == null ? "sem dados" : `${fmtTokens(rep.contextoMedioTurno)} tokens`}</div>
      </div>
      <div className="card">
        <strong>Economia estimada</strong> <small style={{ color: "var(--fg3)" }}>(estimativa; precisa de 5 painéis por lado em cada tipo de tarefa)</small>
        {rep?.economia.length ? (
          <ul>
            {rep.economia.map((e) => (
              <li key={e.taskType}>{TIPO_TAREFA_LABEL[e.taskType] ?? e.taskType}: ≈{fmtUsd(e.economiaEstimada)} por tarefa (precisão ≈{fmtUsd(e.custoPrecisao)} × econômico/equilibrado ≈{fmtUsd(e.custoEco)}; {e.amostrasPrecisao} e {e.amostrasEco} painéis)</li>
            ))}
          </ul>
        ) : <div className="meta">Ainda sem amostra suficiente.</div>}
      </div>
      <div className="card">
        <strong>Qualidade</strong>
        {!active ? <div className="meta">Escolha um workspace para ver a qualidade.</div>
          : qual == null ? <div className="meta">Lendo a qualidade…</div>
          : <div role="status" style={{ color: qual.ok ? undefined : "var(--danger, #c33)" }}>{qual.motivo}</div>}
      </div>
    </>
  );
}

export default function UsageSection() {
  const [period, setPeriod] = useState<Period>("7");
  const [rows, setRows] = useState<UsageRow[]>([]);
  // piloto automático de eficiência: roda ao abrir a tela de Uso (a cada 10 min o ticker também roda)
  useEffect(() => { api.autoajusteRodar().catch(() => null); }, []);
  useEffect(() => {
    // trocar de período rápido: a resposta velha não sobrescreve a nova
    let cancelled = false;
    api.usageRows(sinceDay(period)).then((r) => { if (!cancelled) setRows(r); }).catch(() => { if (!cancelled) setRows([]); });
    return () => { cancelled = true; };
  }, [period]);
  const total = aggregate(rows.map((r) => ({ ...r, cli: "total" })), "cli")[0] ?? { tokIn: 0, tokOut: 0, costUsd: 0, terminals: 0 };
  // todos os dias do período, inclusive os sem uso (barra vazia)
  const byDay = new Map(aggregate(rows, "day").map((d) => [d.key, d]));
  const today = sinceDay("hoje")!;
  const from = sinceDay(period) ?? rows.reduce((m, r) => (r.day < m ? r.day : m), today);
  const days = dayRange(from, today).map((k) => byDay.get(k) ?? { key: k, tokIn: 0, tokOut: 0, costUsd: 0, terminals: 0 });
  const max = Math.max(1, ...days.map((d) => d.tokIn + d.tokOut));
  const table = (by: "cli" | "model" | "effort", title: string) => (
    <div className="card">
      <strong>{title}</strong>
      <table className="tbl">
        <thead><tr><th /><th>Entrada</th><th>Saída</th><th>Custo</th><th>Terminais</th></tr></thead>
        <tbody>
          {aggregate(rows, by).sort((a, b) => b.costUsd - a.costUsd).map((a) => (
            // voz: a "entrada" guarda os segundos de áudio (ver stt.rs) e sem preço não há custo medido
            <tr key={a.key}><td>{by === "cli" ? cliName(a.key) : a.key}</td>
              <td>{by === "cli" && a.key === "stt" ? `${Math.round(a.tokIn / 60)} min` : fmtTokens(a.tokIn)}</td><td>{fmtTokens(a.tokOut)}</td>
              <td>{by === "cli" && a.key === "stt" && a.costUsd === 0 ? `${Math.round(a.tokIn / 60)} min (sem preço)` : `≈${fmtUsd(a.costUsd)}`}</td><td>{a.terminals}</td></tr>
          ))}
        </tbody>
      </table>
    </div>
  );
  return (
    <>
      <h3>Custos e orçamento</h3>
      <div className="row" role="tablist" style={{ marginBottom: 8 }}>
        {PERIODS.map(([p, l]) => <button key={p} role="tab" aria-selected={period === p} className={`tb-btn ${period === p ? "on" : ""}`} onClick={() => setPeriod(p)}>{l}</button>)}
      </div>
      <div className="card">
        <div className="row" style={{ gap: 24 }}>
          <div><strong>{total.terminals}</strong> terminais abertos</div>
          <div><strong>{fmtTokens(total.tokIn)}</strong> entrada</div>
          <div><strong>{fmtTokens(total.tokOut)}</strong> saída</div>
          <div><strong>≈{fmtUsd(total.costUsd)}</strong> estimado</div>
        </div>
        <div className="bars" role="img" aria-label="Tokens por dia">
          {days.map((d) => <div key={d.key} title={`${d.key}: ${fmtTokens(d.tokIn + d.tokOut)} · ≈${fmtUsd(d.costUsd)}`} style={{ height: `${((d.tokIn + d.tokOut) / max) * 100}%` }} />)}
        </div>
      </div>
      {table("cli", "Por IA")}
      {table("model", "Por modelo")}
      {table("effort", "Por esforço")}
      <CodeIndexCard />
      <WasteCard />
      <EconomiaCards />
      <BudgetCard />
    </>
  );
}
