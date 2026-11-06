import { detectErrorBlocks, MAX_BLOCK_LINES, RING_SIZE, type ErrorBlock } from "./errors";
import { capText, SEND_MAX } from "./sendToAgent";
import { useStore } from "../state/store";

/** Fim da saída de cada terminal que o detector olha (um erro longo cabe). */
const TAIL_MAX = 8 * 1024;
/** Espera a saída assentar antes de procurar: um bloco de erro chega em pedaços. */
const SETTLE_MS = 400;

const tails = new Map<string, string>();
const pending = new Map<string, { text: string; timer: number }>();

const key = (b: ErrorBlock) => `${b.kind}\n${b.lines.join("\n")}`;

/** Junta os blocos achados ao anel do painel: sem repetir, o mais recente por último, até `RING_SIZE`. */
export function mergeRing(ring: ErrorBlock[], found: ErrorBlock[]): ErrorBlock[] {
  const seen = new Set(ring.map(key));
  const out = [...ring];
  for (const b of found) {
    if (seen.has(key(b))) continue;
    seen.add(key(b));
    out.push(b);
  }
  return out.slice(-RING_SIZE);
}

/** Saída de terminal chegando: guarda o trecho e, depois de um tempo sem saída, procura erros no fim. */
export function queueErrorScan(paneId: string, chunk: string): void {
  const p = pending.get(paneId) ?? { text: "", timer: 0 };
  p.text = (p.text + chunk).slice(-TAIL_MAX);
  window.clearTimeout(p.timer);
  p.timer = window.setTimeout(() => {
    const text = ((tails.get(paneId) ?? "") + p.text).slice(-TAIL_MAX);
    pending.delete(paneId);
    tails.set(paneId, text);
    const found = detectErrorBlocks(text);
    if (!found.length) return;
    const st = useStore.getState();
    st.patch({ paneErrors: { ...st.paneErrors, [paneId]: mergeRing(st.paneErrors[paneId] ?? [], found) } });
  }, SETTLE_MS);
  pending.set(paneId, p);
}

/** Texto do "Corrija este erro": o bloco, o painel, a pasta e as últimas linhas da tela (cortado em 4 KB). */
export function fixPrompt(pane: { name: string; cwd: string }, block: ErrorBlock, screen: string): string {
  const lines = screen.split("\n").map((l) => l.trimEnd()).filter(Boolean).slice(-20).join("\n");
  const erro = block.lines.slice(0, MAX_BLOCK_LINES).join("\n");
  return capText(
    `Corrija este erro:\n\nPainel: ${pane.name}\nPasta: ${pane.cwd}\n\nErro (${block.kind}):\n\`\`\`\n${erro}\n\`\`\`\n\nÚltimas linhas do terminal:\n\`\`\`\n${lines}\n\`\`\``,
    SEND_MAX,
  );
}

/** Zera os erros de um painel (o envio "Corrigir isto" saiu): o detector também esquece o texto já lido. */
export function clearErrorRing(paneId: string): void {
  tails.delete(paneId);
  const st = useStore.getState();
  st.patch({ paneErrors: { ...st.paneErrors, [paneId]: [] } });
}

/** Painel fechado: solta o que o detector guardou dele. */
export function forgetPaneErrors(paneId: string): void {
  tails.delete(paneId);
  const p = pending.get(paneId);
  if (p) window.clearTimeout(p.timer);
  pending.delete(paneId);
}
