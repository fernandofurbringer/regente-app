import { useEffect, useState } from "react";
import { api } from "./ipc";
import { byHeadroom, totalAccounts } from "./accounts";

/**
 * Conta que o usuário usou por último nesta pasta com a CLI (lida do banco: segue as trocas de conta).
 * `undefined` enquanto lê; `"principal"` quando não há outra (ou a conta foi removida).
 */
export function useLastAccount(wsId: string | null | undefined, cli: string | null | undefined): string | undefined {
  const [conta, setConta] = useState<string | undefined>(undefined);
  useEffect(() => {
    setConta(undefined);
    if (!wsId || !cli) return;
    let live = true;
    api.workspaceLastAccount(wsId, cli)
      .then((c) => { if (live) setConta(c ?? "principal"); })
      .catch(() => { if (live) setConta("principal"); });
    return () => { live = false; };
  }, [wsId, cli]);
  return conta;
}

/** As contas para escolher antes de abrir, da mais folgada para a mais usada; vazio com uma conta só (nada a escolher). */
export function accountChoices(extras: string[] | undefined, used: (account: string) => number | null): string[] {
  if (totalAccounts(extras) < 2) return [];
  return byHeadroom(["principal", ...(extras ?? [])], used);
}

/** Rótulo da opção: a conta e, com leitura do Limites, o quanto já foi usado. */
export function accountOption(conta: string, pct: number | null): string {
  return pct === null ? conta : `${conta} (${Math.round(pct)}% usado)`;
}

/** "principal" vai como null no spawn (a conta padrão do CLI); as outras pelo nome. */
export const spawnAccount = (conta: string | undefined): string | null => (!conta || conta === "principal" ? null : conta);
