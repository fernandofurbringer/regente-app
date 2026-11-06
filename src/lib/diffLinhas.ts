/** Marcas da margem do editor contra a versão do último commit (linhas 1-based, como no Monaco).
 * `removidasApos`: linha depois da qual houve remoção (0 = antes da primeira linha). */
export interface MarcasGit { novas: number[]; alteradas: number[]; removidasApos: number[] }

/** Teto do trecho do meio comparado linha a linha (n × m células); acima dele o trecho todo vira "alterado". */
const TETO = 4_000_000;

export function diffLinhas(base: string, atual: string): MarcasGit {
  const a = base.split(/\r?\n/);
  const b = atual.split(/\r?\n/);
  const m: MarcasGit = { novas: [], alteradas: [], removidasApos: [] };
  let ini = 0;
  while (ini < a.length && ini < b.length && a[ini] === b[ini]) ini++;
  let fa = a.length;
  let fb = b.length;
  while (fa > ini && fb > ini && a[fa - 1] === b[fb - 1]) { fa--; fb--; }
  const A = a.slice(ini, fa);
  const B = b.slice(ini, fb);
  if (!A.length && !B.length) return m;
  if (A.length * B.length > TETO) {
    // ponytail: arquivo enorme com o meio todo diferente vira um bloco "alterado"; Myers se isso incomodar
    for (let j = 0; j < B.length; j++) (A.length ? m.alteradas : m.novas).push(ini + j + 1);
    if (!B.length) m.removidasApos.push(ini);
    return m;
  }
  const n = A.length;
  const k = B.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(k + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = k - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  let i = 0;
  let j = 0;
  let removidas = 0;
  let novas: number[] = [];
  // um bloco de mudança: as primeiras linhas novas "substituem" as removidas (alteradas), o excesso é novo
  const fechar = () => {
    if (!novas.length && removidas) m.removidasApos.push(ini + j);
    const alt = Math.min(removidas, novas.length);
    novas.forEach((ln, x) => (x < alt ? m.alteradas : m.novas).push(ln));
    removidas = 0;
    novas = [];
  };
  while (i < n || j < k) {
    if (i < n && j < k && A[i] === B[j]) { fechar(); i++; j++; }
    else if (j < k && (i >= n || dp[i][j + 1] >= dp[i + 1][j])) { novas.push(ini + j + 1); j++; }
    else { removidas++; i++; }
  }
  fechar();
  return m;
}
