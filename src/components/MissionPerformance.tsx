import { useEffect, useState } from "react";
import { api, on, type MissionPhases } from "../lib/ipc";
import Modal from "./Modal";
import { duracao } from "./VerifyChip";

const ROTULO: Record<string, string> = { planejamento: "planejamento", execucao: "execução", verificacao: "verificação", correcoes: "correções" };

/** Tempo curto para o chip: `12 min` a partir de um minuto, senão `42 s` / `1m10`. */
function tempoDoChip(ms: number): string {
  const min = Math.floor(ms / 60000);
  return min >= 1 ? `${min} min` : duracao(ms);
}

/** Desempenho da missão: barras por fase, tarefa mais lenta e tempo de testes × código. Só leitura. */
export function MissionPerformance({ dados, onClose }: { dados: MissionPhases; onClose(): void }) {
  const maior = Math.max(1, ...dados.per_phase.map((p) => p.wall_ms));
  const execucao = dados.code_ms + dados.test_ms;
  const tokIn = dados.per_phase.reduce((n, p) => n + p.tok_in, 0);
  const tokOut = dados.per_phase.reduce((n, p) => n + p.tok_out, 0);
  const custo = dados.per_phase.reduce((n, p) => n + p.cost_usd, 0);
  return (
    <Modal label="Desempenho da missão" onClose={onClose}>
      <div style={{ padding: 16, minWidth: 420 }}>
        <h2 style={{ margin: "0 0 12px", fontSize: 16 }}>Desempenho da missão</h2>
        {dados.per_phase.map((p) => (
          <div key={p.phase} style={{ display: "grid", gridTemplateColumns: "110px 1fr 80px", gap: 8, alignItems: "center", margin: "6px 0" }}>
            <span>{ROTULO[p.phase] ?? p.phase}</span>
            <div style={{ height: 10, borderRadius: 4, background: "currentColor", opacity: 0.12 }}>
              <div style={{ width: `${(p.wall_ms / maior) * 100}%`, height: 10, borderRadius: 4, background: "currentColor", opacity: 0.6 }} />
            </div>
            <span style={{ textAlign: "right" }}>{p.wall_ms > 0 ? duracao(p.wall_ms) : "—"}</span>
          </div>
        ))}
        <p style={{ margin: "12px 0 4px" }}>
          Tarefa mais lenta: {dados.slowest_task ? `${dados.slowest_task.task_id} · ${duracao(dados.slowest_task.wall_ms)}` : "—"}
        </p>
        <p style={{ margin: "4px 0" }}>
          Testes × código: {execucao > 0
            ? `testes ${duracao(dados.test_ms)} (${Math.round((dados.test_ms / execucao) * 100)}%) · código ${duracao(dados.code_ms)} (${Math.round((dados.code_ms / execucao) * 100)}%)`
            : "—"}
        </p>
        <p style={{ margin: "4px 0", opacity: 0.8 }}>
          Tokens: {tokIn.toLocaleString("pt-BR")} entrada · {tokOut.toLocaleString("pt-BR")} saída · US$ {custo.toFixed(2).replace(".", ",")}
        </p>
      </div>
    </Modal>
  );
}

/** Chip da missão na barra: `Fase: execução · 12 min` enquanto uma fase roda. Clique abre o desempenho. */
export function PhaseChip({ missionId }: { missionId: string }) {
  const [dados, setDados] = useState<MissionPhases | null>(null);
  const [aberto, setAberto] = useState(false);
  useEffect(() => {
    let vivo = true;
    const carrega = () => {
      api.missionPhases(missionId).then((d) => { if (vivo) setDados(d); }).catch(() => { /* chip é informativo */ });
    };
    carrega();
    const tick = setInterval(carrega, 15000);
    const un = on<{ missionId: string }>("tasks-changed", (e) => {
      if (e.missionId === missionId) carrega();
    });
    return () => {
      vivo = false;
      clearInterval(tick);
      un.then((f) => f());
    };
  }, [missionId]);

  // sem fase medida, a barra não ganha chip
  if (!dados || (!dados.atual && dados.per_phase.every((p) => p.wall_ms === 0))) return null;
  const atual = dados.atual;
  const texto = atual
    ? `Fase: ${ROTULO[atual.phase] ?? atual.phase} · ${tempoDoChip(Math.max(0, Date.now() - atual.started_at))}`
    : "Desempenho";
  return (
    <>
      <button className="btn" title="Desempenho da missão: tempo, tokens e custo por fase" onClick={() => setAberto(true)}>{texto}</button>
      {aberto && <MissionPerformance dados={dados} onClose={() => setAberto(false)} />}
    </>
  );
}
