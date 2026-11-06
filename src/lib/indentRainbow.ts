/** Uma faixa do indent-rainbow: colunas 1-based do Monaco (fim exclusivo) e o nível (0..3, as quatro cores se repetem). */
export interface Faixa { inicio: number; fim: number; nivel: number }

/** Faixas da indentação de uma linha: cada tab é um nível; espaços contam de `tab` em `tab`.
 * Sobra de espaços menor que um nível não ganha faixa. */
export function faixasIndentacao(linha: string, tab: number): Faixa[] {
  const out: Faixa[] = [];
  const passo = Math.max(1, tab);
  let col = 0;
  let nivel = 0;
  while (col < linha.length) {
    if (linha[col] === "\t") {
      out.push({ inicio: col + 1, fim: col + 2, nivel: nivel++ % 4 });
      col++;
      continue;
    }
    let n = 0;
    while (n < passo && linha[col + n] === " ") n++;
    if (n < passo) break;
    out.push({ inicio: col + 1, fim: col + 1 + passo, nivel: nivel++ % 4 });
    col += passo;
  }
  return out;
}
