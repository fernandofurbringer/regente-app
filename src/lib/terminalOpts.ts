/** Linhas de histórico do xterm: padrão 5000, faixa 1000 a 20000 (espelho de `settings::clamp_scrollback`). */
export function scrollbackLines(n: number | undefined): number {
  if (n === undefined || !Number.isFinite(n)) return 5000;
  return Math.min(20000, Math.max(1000, Math.round(n)));
}

/** Espera antes da tentativa `n` (1 a 120) do attach: 40, 80, 160 e 320 ms; depois 500 ms. */
export function attachRetryDelay(n: number): number {
  return n <= 4 ? 40 * 2 ** (n - 1) : 500;
}

/** Tentativas de attach antes de desistir (≈ 60 s com o intervalo de 500 ms). */
export const ATTACH_MAX_TRIES = 120;
