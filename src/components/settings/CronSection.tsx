import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { buildExpr, fmtWhen, parseExpr, PRESETS, WEEKDAYS, type Freq } from "../../lib/cron";
import { api, errMsg, on, type CronJob } from "../../lib/ipc";
import { useStore, wsOf } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const FREQS: { id: Freq; label: string }[] = [
  { id: "daily", label: "Diário" }, { id: "weekly", label: "Semanal" }, { id: "monthly", label: "Mensal" }, { id: "custom", label: "Personalizado" },
];

export default function CronSection() {
  const active = useStore((s) => s.active);
  const ws = wsOf(active);
  const [jobs, setJobs] = useState<CronJob[] | null>(null);
  const [edit, setEdit] = useState<CronJob | "new" | null>(null);
  const load = () => (ws ? api.cronList(ws.id).then(setJobs).catch((e) => { setJobs([]); toast(e); }) : Promise.resolve());
  useEffect(() => { setJobs(null); load(); }, [active]);
  // the ticker runs jobs while this screen is open: refresh "Última" / "Último erro"
  useEffect(() => { const un = on("cron-ran", () => { load(); }); return () => { un.then((f) => f()); }; }, [active]);

  if (!ws) return <><h3>Agendamentos</h3><div className="meta">Abra um workspace para ver os agendamentos.</div></>;

  async function toggle(j: CronJob) {
    try {
      await api.cronSave({ ...j, enabled: !j.enabled });
      await load();
    } catch (e) { toast(e); }
  }
  async function runNow(j: CronJob) {
    try {
      await api.cronRunNow(j.id);
      useStore.getState().toast(`"${j.name}" disparado`, "ok");
      setTimeout(() => load(), 1500);
    } catch (e) { toast(e); }
  }
  async function remove(j: CronJob) {
    if (!(await ask(`Excluir o agendamento "${j.name}"?`, { title: "Excluir agendamento", kind: "warning" }))) return;
    try {
      await api.cronDelete(j.id);
      await load();
    } catch (e) { toast(e); }
  }

  return (
    <>
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 6 }}>
        <h3 style={{ margin: 0 }}>Agendamentos de {ws.name}</h3>
        <button className="btn primary" onClick={() => setEdit("new")}>Novo agendamento</button>
      </div>
      <p className="meta">Só roda com o Regente aberto.</p>
      {jobs === null ? <div className="meta">Carregando agendamentos…</div>
        : jobs.length === 0 ? <div className="meta">Nenhum agendamento neste workspace.</div>
        : (
          <table className="tbl">
            <thead><tr><th>Nome</th><th>Próxima</th><th>Última</th><th>Último erro</th><th>Ativo</th><th aria-label="Ações" /></tr></thead>
            <tbody>
              {jobs.map((j) => (
                <tr key={j.id}>
                  <td>{j.name}<div className="meta">{j.expr} · {j.target.startsWith("pane:") ? "painel existente" : "novo painel"}</div></td>
                  <td>{j.enabled ? fmtWhen(j.nextRun) : "—"}</td>
                  <td>{fmtWhen(j.lastRun)}</td>
                  <td style={{ color: j.lastError ? "var(--warn)" : undefined }}>{j.lastError ?? ""}</td>
                  <td><label><input type="checkbox" checked={j.enabled} aria-label={`Ativo: ${j.name}`} onChange={() => toggle(j)} /></label></td>
                  <td className="row">
                    <button className="btn" onClick={() => runNow(j)}>Rodar agora</button>
                    <button className="btn" onClick={() => setEdit(j)}>Editar</button>
                    <button className="btn danger" onClick={() => remove(j)}>Excluir</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      {edit && <JobForm key={edit === "new" ? "new" : edit.id} workspaceId={ws.id} job={edit === "new" ? null : edit} onClose={() => setEdit(null)} onSaved={() => { setEdit(null); load(); }} />}
    </>
  );
}

function JobForm({ workspaceId, job, onClose, onSaved }: { workspaceId: string; job: CronJob | null; onClose(): void; onSaved(): void }) {
  const init = job ? parseExpr(job.expr) : { freq: "daily" as Freq, time: "09:00", days: [1], monthDay: 1 };
  const [name, setName] = useState(job?.name ?? "");
  const [freq, setFreq] = useState<Freq>(init.freq);
  const [time, setTime] = useState(init.time);
  const [days, setDays] = useState<number[]>(init.days);
  const [monthDay, setMonthDay] = useState(init.monthDay);
  const [raw, setRaw] = useState(job?.expr ?? "0 9 * * *");
  const [prompt, setPrompt] = useState(job?.prompt ?? "");
  const [target, setTarget] = useState<"new" | "pane">(job?.target.startsWith("pane:") ? "pane" : "new");
  const [paneId, setPaneId] = useState(job?.target.startsWith("pane:") ? job.target.slice(5) : "");
  const [cli, setCli] = useState(job?.spawnJson ? (JSON.parse(job.spawnJson) as { cli: string }).cli : "claude");
  const [model, setModel] = useState(job?.spawnJson ? (JSON.parse(job.spawnJson) as { model?: string | null }).model ?? "" : "");
  const [effort, setEffort] = useState(job?.spawnJson ? (JSON.parse(job.spawnJson) as { effort?: string | null }).effort ?? "" : "");
  const [preview, setPreview] = useState<{ ok: string[] } | { error: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const providers = useStore((s) => s.providers);
  const panes = useStore((s) => s.panes);
  const openPanes = Object.values(panes).filter((p) => p.workspaceId === workspaceId && p.closedAt === null);

  let expr = "";
  try {
    expr = freq === "custom" ? raw.trim() : buildExpr(freq, time, days, monthDay);
  } catch {
    expr = ""; // a hora ou os dias inválidos aparecem na prévia
  }

  useEffect(() => {
    if (!expr) { setPreview({ error: freq === "custom" ? "escreva a expressão" : "confira hora e dias" }); return; }
    let alive = true;
    const t = setTimeout(() => {
      api.cronPreview(expr)
        .then((ms) => { if (alive) setPreview({ ok: ms.map((m) => fmtWhen(m)) }); })
        .catch((e) => { if (alive) setPreview({ error: errMsg(e) }); });
    }, 250);
    return () => { alive = false; clearTimeout(t); };
  }, [expr, freq]);

  async function save() {
    if (busy) return;
    setBusy(true);
    try {
      const spawnJson = target === "new" ? JSON.stringify({ cli, model: model || null, effort: effort || null }) : null;
      await api.cronSave({
        id: job?.id ?? "", workspaceId, name: name.trim(), expr, prompt,
        target: target === "pane" ? `pane:${paneId}` : "novo", spawnJson,
        enabled: job?.enabled ?? true, lastRun: job?.lastRun ?? null, nextRun: null, lastError: null,
      });
      onSaved();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  const cliDef = providers?.clis.find((c) => c.id === cli);
  const toggleDay = (n: number) => setDays((d) => (d.includes(n) ? d.filter((x) => x !== n) : [...d, n]));
  return (
    <div className="card" role="group" aria-label={job ? "Editar agendamento" : "Novo agendamento"} style={{ marginTop: 12 }}>
      <h4 style={{ marginTop: 0 }}>{job ? "Editar agendamento" : "Novo agendamento"}</h4>
      <label className="field"><span>Nome</span><input value={name} onChange={(e) => setName(e.target.value)} /></label>
      <div className="field">
        <span>Atalhos</span>
        <div className="row">{PRESETS.map((p) => <button key={p.expr} type="button" className="chip" onClick={() => { setFreq("custom"); setRaw(p.expr); }}>{p.label}</button>)}</div>
      </div>
      <div className="field">
        <span>Frequência</span>
        <div className="row">
          {FREQS.map((f) => <button key={f.id} type="button" className="btn" aria-pressed={freq === f.id} onClick={() => setFreq(f.id)}>{f.label}</button>)}
        </div>
      </div>
      {freq === "weekly" && (
        <div className="row" role="group" aria-label="Dias da semana">
          {WEEKDAYS.map((w) => <button key={w.n} type="button" className="chip" aria-pressed={days.includes(w.n)} onClick={() => toggleDay(w.n)}>{w.label}</button>)}
        </div>
      )}
      {freq === "monthly" && (
        <label className="field"><span>Dia do mês</span><input type="number" min={1} max={31} value={monthDay} onChange={(e) => setMonthDay(Number(e.target.value))} /></label>
      )}
      {freq === "custom" ? (
        <label className="field"><span>Expressão cron (5 campos)</span><input value={raw} onChange={(e) => setRaw(e.target.value)} /></label>
      ) : (
        <label className="field"><span>Hora</span><input type="time" value={time} onChange={(e) => setTime(e.target.value)} /></label>
      )}
      <div className="meta" aria-live="polite">
        {preview === null ? "…" : "error" in preview ? `Expressão inválida: ${preview.error}` : `Próximas execuções: ${preview.ok.join(" · ")}`}
      </div>
      <label className="field"><span>Prompt</span><textarea rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} /></label>
      <div className="field">
        <span>Alvo</span>
        <div className="row">
          <label className="row"><input type="radio" name="cron-target" checked={target === "new"} onChange={() => setTarget("new")} /> Novo painel</label>
          <label className="row"><input type="radio" name="cron-target" checked={target === "pane"} onChange={() => setTarget("pane")} /> Painel existente</label>
        </div>
      </div>
      {target === "new" ? (
        <div className="row">
          <label className="field"><span>IA</span>
            <select value={cli} onChange={(e) => setCli(e.target.value)}>
              {(providers?.clis ?? []).map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
            </select>
          </label>
          <label className="field"><span>Modelo</span><input list="cron-models" value={model} placeholder="padrão" onChange={(e) => setModel(e.target.value)} />
            <datalist id="cron-models">{(cliDef?.models ?? []).map((m) => <option key={m} value={m} />)}</datalist>
          </label>
          <label className="field"><span>Quanto pensa</span>
            <select value={effort} onChange={(e) => setEffort(e.target.value)}>
              <option value="">padrão</option>
              {(cliDef?.efforts ?? []).map((x) => <option key={x} value={x}>{x}</option>)}
            </select>
          </label>
          <p className="meta" role="note">Roda sem você: se a IA pedir confirmação, o painel fica esperando (com a permissão "Padrão do CLI", se o seu CLI pergunta). Prefira prompts que não pedem nada.</p>
        </div>
      ) : (
        <label className="field"><span>Painel</span>
          <select value={paneId} onChange={(e) => setPaneId(e.target.value)}>
            <option value="">escolha um painel aberto</option>
            {openPanes.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
        </label>
      )}
      <div className="actions">
        <button type="button" className="btn" onClick={onClose}>Cancelar</button>
        <button type="button" className="btn primary" disabled={busy || preview === null || !name.trim() || !expr || (target === "pane" && !paneId) || "error" in preview} onClick={save}>Salvar</button>
      </div>
    </div>
  );
}
