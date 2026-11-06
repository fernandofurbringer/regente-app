/** Limites da hibernação automática (o backend recorta do mesmo jeito). Entrada inválida volta ao padrão. */
export function clampIdleMinutes(v: number): number {
  return Number.isFinite(v) ? Math.min(1440, Math.max(1, Math.round(v))) : 10;
}

export function clampResidentFloor(v: number): number {
  return Number.isFinite(v) ? Math.min(16, Math.max(0, Math.round(v))) : 2;
}
