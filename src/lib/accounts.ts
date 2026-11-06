/** Contas de um CLI: a "principal" é implícita e `extras` são as cadastradas em Configurações → IAs. */
export const totalAccounts = (extras: string[] | undefined): number => 1 + (extras?.length ?? 0);

/**
 * O selo de conta só ajuda quando há o que distinguir: 2 ou mais contas para o CLI. E a conta do painel precisa
 * existir (uma conta removida depois não deve deixar o selo órfão).
 */
export function showAccountBadge(extras: string[] | undefined, paneAccount: string | null | undefined): boolean {
  if (totalAccounts(extras) < 2) return false;
  const conta = paneAccount || "principal";
  return conta === "principal" || (extras ?? []).includes(conta);
}

/** As outras contas do CLI (a principal entra na lista), menos a do painel. */
export function otherAccounts(extras: string[] | undefined, paneAccount: string | null | undefined): string[] {
  const atual = paneAccount || "principal";
  return ["principal", ...(extras ?? [])].filter((c) => c !== atual);
}

/**
 * Da conta mais folgada para a mais usada, pelo maior uso entre as janelas do Limites. Sem leitura, a conta fica no
 * meio (50%): melhor que uma quase cheia, pior que uma que se sabe vazia. Empate mantém a ordem recebida.
 */
export function byHeadroom(accounts: string[], used: (account: string) => number | null): string[] {
  const peso = (c: string) => used(c) ?? 50;
  return [...accounts].sort((a, b) => peso(a) - peso(b));
}
