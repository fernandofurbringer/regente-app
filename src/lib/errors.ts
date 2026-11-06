// Detector de blocos de erro na saída de terminal (A1.3, parte pura). Reconhece erro de Rust, TypeScript,
// Traceback do Python, FAIL de Jest/Vitest, npm ERR!, `Error:` com stack e erro do PowerShell.
// `startLine` é o índice (0-based) da linha de início no texto, depois de remover ANSI e CR.

export type ErrorKind = "rust" | "ts" | "python" | "jest" | "npm" | "stack" | "powershell";

export interface ErrorBlock {
  kind: ErrorKind;
  lines: string[];
  startLine: number;
}

/** Limite de linhas por bloco. */
export const MAX_BLOCK_LINES = 40;

/** Quantos blocos o anel guarda por painel. */
export const RING_SIZE = 5;

type Acao = "incluir" | "parar" | "fim";

interface Regra {
  kind: ErrorKind;
  inicio: RegExp;
  /** Decide o que fazer com a linha seguinte ao início do bloco. */
  consome: (linha: string) => Acao;
}

const indentada = (l: string) => /^\s/.test(l);
const vazia = (l: string) => l.trim() === "";

const REGRAS: Regra[] = [
  {
    kind: "rust",
    inicio: /^error(\[E\d+\])?:/,
    consome: (l) => (vazia(l) ? "parar" : "incluir"),
  },
  {
    kind: "ts",
    inicio: /\bTS\d{4}\b/,
    consome: (l) => (indentada(l) && !vazia(l) ? "incluir" : "parar"),
  },
  {
    kind: "python",
    inicio: /^Traceback \(most recent call last\):/,
    consome: (l) => {
      if (vazia(l)) return "parar";
      // a última linha do traceback é a exceção, sem recuo
      return indentada(l) ? "incluir" : "fim";
    },
  },
  {
    kind: "jest",
    inicio: /^\s*FAIL\s/,
    consome: (l) =>
      /^\s*(PASS |FAIL |Test Suites:|Tests:|Tests\s|Test Files|Snapshots:|Time:|Ran all)/.test(l) ? "parar" : "incluir",
  },
  {
    kind: "npm",
    inicio: /^npm (ERR!|error)/,
    consome: (l) => (/^npm (ERR!|error)/.test(l) ? "incluir" : "parar"),
  },
  {
    kind: "powershell",
    inicio: /^At line:\d+ char:\d+|^\s*\+ CategoryInfo/,
    consome: (l) => (indentada(l) || /^(\+|At line:)/.test(l) ? "incluir" : "parar"),
  },
  {
    kind: "stack",
    inicio: /^\w*(Error|Exception):/,
    consome: (l) => (indentada(l) && !vazia(l) ? "incluir" : "parar"),
  },
];

const ANSI = /\x1b\[[0-9;?]*[A-Za-z]/g;

export function detectErrorBlocks(texto: string): ErrorBlock[] {
  const linhas = texto.replace(ANSI, "").split("\n").map((l) => l.replace(/\r$/, ""));
  const out: ErrorBlock[] = [];
  let i = 0;
  while (i < linhas.length) {
    const regra = REGRAS.find((r) => r.inicio.test(linhas[i]));
    if (!regra) {
      i++;
      continue;
    }
    const startLine = i;
    const bloco = [linhas[i]];
    i++;
    while (i < linhas.length && bloco.length < MAX_BLOCK_LINES) {
      const acao = regra.consome(linhas[i]);
      if (acao === "parar") break;
      bloco.push(linhas[i]);
      i++;
      if (acao === "fim") break;
    }
    while (bloco.length > 1 && vazia(bloco[bloco.length - 1])) bloco.pop();
    out.push({ kind: regra.kind, lines: bloco, startLine });
  }
  return out;
}

/** Últimos blocos de erro de cada painel (até `RING_SIZE`). */
export class ErrorRing {
  private readonly porPainel = new Map<string, ErrorBlock[]>();
  private readonly tamanho: number;

  constructor(tamanho: number = RING_SIZE) {
    this.tamanho = tamanho;
  }

  push(paneId: string, bloco: ErrorBlock): void {
    const lista = [...(this.porPainel.get(paneId) ?? []), bloco];
    this.porPainel.set(paneId, lista.slice(-this.tamanho));
  }

  last(paneId: string): ErrorBlock | undefined {
    const lista = this.porPainel.get(paneId);
    return lista?.[lista.length - 1];
  }

  list(paneId: string): ErrorBlock[] {
    return [...(this.porPainel.get(paneId) ?? [])];
  }

  clear(paneId: string): void {
    this.porPainel.delete(paneId);
  }
}
