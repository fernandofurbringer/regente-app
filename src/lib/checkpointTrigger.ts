import { useEffect } from "react";
import { api, errMsg, type Mission, type PaneRow } from "./ipc";
import { useStore } from "../state/store";

/** Intervalo dos checkpoints enquanto algum agente do escopo está trabalhando. */
export const CHECKPOINT_EVERY_MS = 10 * 60 * 1000;

/**
 * Escopo dos checkpoints de um painel: a missão, se usa worktree; senão o workspace, mas só com `checkpoints.raiz` ligado.
 * `null` = o painel não tem checkpoint automático: workspace remoto (`ssh://`) ou em modo sensível.
 */
export function scopeOf(pane: PaneRow, missions: Mission[], raiz: boolean, sensitive: string[] = []): string | null {
  if (pane.cwd?.startsWith("ssh://") || sensitive.includes(pane.workspaceId)) return null;
  const m = pane.missionId ? missions.find((x) => x.id === pane.missionId) : undefined;
  if (m?.worktreePath) return m.id;
  return raiz ? pane.workspaceId : null;
}

/** Checkpoint manual (palette, linha do tempo): mostra o resultado; "nada mudou" é um aviso, não um erro. */
export function checkpointAgora(scope: string, pane?: string): void {
  const toast = useStore.getState().toast;
  api.checkpointTake(scope, pane).then(
    (cp) => toast(cp ? `Checkpoint ${cp.n} criado (${cp.totalFiles} arquivo${cp.totalFiles === 1 ? "" : "s"}).${cp.largeSkipped ? ` ${cp.largeSkipped} arquivo${cp.largeSkipped === 1 ? "" : "s"} grande${cp.largeSkipped === 1 ? "" : "s"} ficaram fora do checkpoint.` : ""}` : "Nada mudou desde o último checkpoint.", cp ? "ok" : "info"),
    (e) => toast(errMsg(e), "erro"),
  );
}

/**
 * Checkpoints automáticos: quando um agente vai de Trabalhando para Ocioso, e a cada 10 min enquanto algum agente
 * do escopo trabalha. O backend pula o que não mudou, então chamar de mais não cria checkpoint vazio.
 */
export function useCheckpointTrigger(): void {
  useEffect(() => {
    // falha de checkpoint automático não tem tela: fica no console para o diagnóstico
    const take = (scope: string, pane?: string) => {
      api.checkpointTake(scope, pane).catch((e) => console.warn("checkpoint automático falhou", scope, e));
    };
    const unsub = useStore.subscribe((s, prev) => {
      if (s.panes === prev.panes) return;
      const raiz = !!s.settings?.checkpointsRaiz;
      const sensitive = s.settings?.sensitiveWorkspaces ?? [];
      const missions = Object.values(s.missions).flat();
      for (const p of Object.values(s.panes)) {
        if (p.kind !== "agent" || prev.panes[p.id]?.status !== "trabalhando" || p.status !== "ocioso") continue;
        const scope = scopeOf(p, missions, raiz, sensitive);
        if (scope) take(scope, p.name);
      }
    });
    const timer = setInterval(() => {
      const s = useStore.getState();
      const raiz = !!s.settings?.checkpointsRaiz;
      const sensitive = s.settings?.sensitiveWorkspaces ?? [];
      const missions = Object.values(s.missions).flat();
      const working = new Map<string, string>();
      for (const p of Object.values(s.panes)) {
        if (p.kind !== "agent" || p.status !== "trabalhando") continue;
        const scope = scopeOf(p, missions, raiz, sensitive);
        if (scope && !working.has(scope)) working.set(scope, p.name);
      }
      working.forEach((pane, scope) => take(scope, pane));
    }, CHECKPOINT_EVERY_MS);
    return () => {
      unsub();
      clearInterval(timer);
    };
  }, []);
}
