import { ask } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, errMsg, type CliDef, type PaneRow } from "../lib/ipc";
import { rememberPick } from "../lib/lastAgent";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { terms } from "./TerminalPane";

/** Papel longo (piloto, maestro, planner ou painel do usuário sem papel): o que tem cache de 1 h. */
const isLongRole = (role: string | null) => !role || role === "piloto" || role === "maestro" || role === "planner";

/** Modelo e esforço de um painel de IA. Qualquer troca reinicia o painel retomando a sessão; sem sessão, só com confirmação. */
export default function ModelSwitch({ pane, cli, onClose }: { pane: PaneRow; cli: CliDef; onClose(): void }) {
  const [busy, setBusy] = useState(false);
  const hibernated = pane.status === "hibernado";
  const hasSession = !!pane.sessionId;
  const { toast, upsertPane } = useStore.getState();

  const blocked = (): string | null => (busy ? "Aguarde" : null);

  async function choose(model: string | null, effort: string | null) {
    if (model === pane.model && effort === pane.effort) return;
    if (blocked()) return;
    // só o esforço, com sessão, reinicia sem perguntar (RM-12); sem sessão o histórico fica para trás, então pergunta
    const trocaModelo = model !== pane.model;
    if (!hibernated && (trocaModelo || !hasSession)) {
      const what = [trocaModelo ? `modelo ${model ?? "padrão"}` : null, effort !== pane.effort ? `esforço ${effort ?? "padrão"}` : null]
        .filter(Boolean)
        .join(" e ");
      // cache quente (só Claude): trocar de modelo faz o próximo turno reler tudo (a janela é 1 h nos papéis longos, 5 min nos curtos)
      const idle = trocaModelo && hasSession && pane.cli === "claude" ? await api.paneIdleSecs(pane.id).catch(() => null) : null;
      const quente = idle !== null && idle < (isLongRole(pane.role) ? 3600 : 300);
      const cache = quente ? " Trocar de modelo agora faz o próximo turno reler todo o contexto sem cache." : "";
      const text = hasSession
        ? `Reiniciar o painel com ${what}?${cache} A conversa é retomada, mas o que estiver em andamento para.${quente ? " Continuar?" : ""}`
        : `Esta conversa não pode ser retomada. Reiniciar do zero com ${what}? O histórico atual fica para trás.`;
      const ok = await ask(text, { title: "Reiniciar painel", kind: "warning" });
      if (!ok) return;
    }
    setBusy(true);
    try {
      // a tela só serve para mostrar enquanto o painel reinicia
      const frame = hibernated ? "" : terms.get(pane.id)?.screen() ?? "";
      const row = await api.paneSwitchModel(pane.id, model, effort, frame, !hasSession);
      upsertPane(row);
      if (pane.cli) rememberPick(pane.cli, { model: row.model, effort: row.effort });
      toast(hibernated ? "Escolha salva: vale na próxima vez que o painel acordar. Ao acordar, o cache estará frio: o primeiro turno relê o contexto." : "Painel reiniciado com a nova escolha", "ok");
      onClose();
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  const choice = (on: boolean, label: string, model: string | null, effort: string | null, key: string) => {
    const why = blocked();
    return (
      <button key={key} className="btn" aria-pressed={on} disabled={!!why && !on} title={why ?? undefined} onClick={() => void choose(model, effort)}>
        {label}
      </button>
    );
  };

  return (
    <Modal label="Modelo e esforço" onClose={onClose} style={{ width: "min(480px, 92vw)", display: "flex", flexDirection: "column", gap: 10 }}>
      <h2>Modelo e esforço</h2>
      <p className="meta">
        Trocar o modelo ou o esforço reinicia o painel, retomando a sessão. O comando /model não é usado: ele gravaria o padrão de todas as sessões.
      </p>
      {cli.models.length > 0 && (
        <>
          <h3>Modelo</h3>
          <div className="row" style={{ flexWrap: "wrap", gap: 6 }}>
            {cli.models.map((m) => choice(m === pane.model, m, m, pane.effort, `m-${m}`))}
          </div>
        </>
      )}
      {cli.efforts.length > 0 && (
        <>
          <h3>Esforço</h3>
          <div className="row" style={{ flexWrap: "wrap", gap: 6 }}>
            {choice(pane.effort === null, "padrão", pane.model, null, "e-padrao")}
            {cli.efforts.map((e) => choice(e === pane.effort, e, pane.model, e, `e-${e}`))}
          </div>
        </>
      )}
    </Modal>
  );
}

/** Abre o diálogo do painel guardado em `modelPane` (chip do cabeçalho ou paleta). Fica montado em cada janela. */
export function ModelSwitchDialog() {
  const paneId = useStore((s) => s.modelPane);
  const pane = useStore((s) => (paneId ? s.panes[paneId] : undefined));
  const cli = useStore((s) => (pane?.cli ? s.providers?.clis.find((c) => c.id === pane.cli) : undefined));
  if (!paneId || !pane || !cli) return null;
  return <ModelSwitch pane={pane} cli={cli} onClose={() => useStore.getState().patch({ modelPane: null })} />;
}
