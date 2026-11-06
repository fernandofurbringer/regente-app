import type { UpdateStatus } from "./ipc";

/** Texto do estado da atualização (Configurações → Atualizações). */
export function statusText(s: UpdateStatus): string {
  if (s.state === "nao-configurado") return "Sem canal configurado. Informe a URL do latest.json abaixo.";
  if (s.state === "atualizado") return "Você está na versão mais recente.";
  return `Versão ${s.version ?? "?"} disponível.`;
}

/** Percentual do download; null quando o servidor não informou o tamanho. */
export function progressPercent(downloaded: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.round((downloaded * 100) / total));
}
