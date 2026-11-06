// Registro de servidores de desenvolvimento (A1.7, RM-44): um item por painel (terminal PTY ou painel exec) com URL
// local anunciada. Funções puras: a loja guarda o mapa, o chip "Servidores (n)" lê.

import { findPortInUse } from "./links";

export type DevSource = "terminal" | "exec";

export interface DevServer {
  paneId: string;
  url: string;
  /** quando o servidor foi anunciado (ms); some quando o painel fecha */
  since: number;
  source: DevSource;
  /** "Parar" foi pedido: a entrada sai quando o painel fica ocioso (ou 5 s depois, em DevServers) */
  parando?: boolean;
}

export type DevMap = Record<string, DevServer>;

/** Registra (ou atualiza) o servidor do painel. Mesma URL no mesmo painel mantém o `since` antigo. */
export function registrar(map: DevMap, e: DevServer): DevMap {
  const atual = map[e.paneId];
  if (atual && atual.url === e.url) return map;
  return { ...map, [e.paneId]: e };
}

/** Marca o servidor do painel como parando (fica no chip até o painel parar de imprimir a URL). */
export function marcarParando(map: DevMap, paneId: string): DevMap {
  const atual = map[paneId];
  if (!atual || atual.parando) return map;
  return { ...map, [paneId]: { ...atual, parando: true } };
}

/** Tira o servidor do painel que fechou. Sem entrada, devolve o mesmo mapa. */
export function remover(map: DevMap, paneId: string): DevMap {
  if (!(paneId in map)) return map;
  const { [paneId]: _fora, ...resto } = map;
  return resto;
}

/** Porta de uma URL local (null se não tiver). */
export function portaDaUrl(url: string): number | null {
  try {
    const p = new URL(url).port;
    return p ? Number(p) : null;
  } catch {
    return null;
  }
}

/** Dica para um erro de porta em uso: quem está com a porta (outro painel) ou um processo fora do Regente. */
export function dicaDePorta(texto: string, map: DevMap, paneId: string, nomeDoPainel: (id: string) => string): string | null {
  const porta = findPortInUse(texto);
  if (porta === null) return null;
  const dono = Object.values(map).find((e) => e.paneId !== paneId && portaDaUrl(e.url) === porta);
  return dono
    ? `Porta ${porta} em uso por outro painel: ${nomeDoPainel(dono.paneId)}`
    : `Porta ${porta} em uso por outro processo. Feche esse processo ou use outra porta.`;
}
