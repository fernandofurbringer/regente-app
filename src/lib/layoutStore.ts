import { useMemo } from "react";
import { useStore } from "../state/store";
import { SHELL_DEFAULT, clampShell, readGridPrefs, readShellPrefs, writeGridPrefs, writeShellPrefs, type GridPrefs, type ShellPrefs } from "./layout";

/** Preferências da grade do workspace: as da sessão, senão as salvas. */
export function gridPrefsOf(ws: string): GridPrefs {
  return useStore.getState().gridPrefs[ws] ?? readGridPrefs(ws);
}

/** Troca as preferências da grade; `salvar` grava no armazenamento local (no fim do arrasto, não a cada movimento). */
export function setGridPrefs(ws: string, p: GridPrefs, salvar = true): void {
  const st = useStore.getState();
  st.patch({ gridPrefs: { ...st.gridPrefs, [ws]: p } });
  if (salvar) writeGridPrefs(ws, p);
}

export function shellPrefsNow(): ShellPrefs {
  return useStore.getState().shellPrefs ?? readShellPrefs();
}

export function setShellPrefs(p: ShellPrefs, salvar = true): void {
  useStore.getState().patch({ shellPrefs: p });
  if (salvar) writeShellPrefs(p);
}

export function useGridPrefs(ws: string): GridPrefs {
  const s = useStore((st) => st.gridPrefs[ws]);
  return useMemo(() => s ?? readGridPrefs(ws), [s, ws]);
}

export function useShellPrefs(): ShellPrefs {
  const s = useStore((st) => st.shellPrefs);
  return useMemo(() => s ?? readShellPrefs(), [s]);
}

/** Largura no começo do arrasto de cada coluna do workspace (um arrasto por vez). Fora do componente: os handlers
 *  são recriados a cada render durante o arrasto e precisam do mesmo ponto de partida. */
const inicioShell: Partial<Record<keyof ShellPrefs, number>> = {};

/** Handlers do divisor da árvore de arquivos. */
export function shellDrag(k: keyof ShellPrefs) {
  return {
    onStart: () => { inicioShell[k] = shellPrefsNow()[k]; },
    onMove: (d: number) => setShellPrefs({ ...shellPrefsNow(), [k]: clampShell(k, (inicioShell[k] ?? shellPrefsNow()[k]) + d) }, false),
    onEnd: () => { delete inicioShell[k]; setShellPrefs(shellPrefsNow()); },
    onReset: () => setShellPrefs({ ...shellPrefsNow(), [k]: SHELL_DEFAULT[k] }),
  };
}
