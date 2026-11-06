import type { ChangedFile } from "./ipc";

const LANGS: Record<string, string> = {
  ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript", mjs: "javascript", cjs: "javascript",
  json: "json", md: "markdown", css: "css", scss: "scss", html: "html", rs: "rust", py: "python", go: "go", java: "java",
  cs: "csharp", sql: "sql", xml: "xml", yml: "yaml", yaml: "yaml", toml: "ini", sh: "shell", ps1: "powershell",
};

/** Linguagem do Monaco pela extensão do arquivo (texto puro quando não reconhece). */
export function langOf(path: string): string {
  const ext = path.toLowerCase().split(".").pop() ?? "";
  return LANGS[ext] ?? "plaintext";
}

/** Arquivos aceitos na revisão: caminho -> hash do conteúdo que foi aceito. Se o arquivo mudar, o hash difere e ele volta a pendente. */
export type Accepted = Record<string, string>;

const KEY = (missionId: string) => `regente.revisao.${missionId}`;

/** Aceitos guardados no navegador (sobrevivem a reiniciar o app). Falha de leitura não quebra a revisão. */
export function acceptedOf(missionId: string): Accepted {
  try {
    const v = JSON.parse(localStorage.getItem(KEY(missionId)) ?? "{}");
    return v && typeof v === "object" && !Array.isArray(v) ? v : {};
  } catch {
    return {};
  }
}

/** Grava os aceitos da missão. Sem armazenamento disponível a revisão vale só nesta sessão. */
export function storeAccepted(missionId: string, acc: Accepted): void {
  try {
    localStorage.setItem(KEY(missionId), JSON.stringify(acc));
  } catch {
    /* sem armazenamento: o aceite segue na memória da janela */
  }
}

/** Arquivos da missão que ainda não foram aceitos com este conteúdo. */
export function unreviewed(files: ChangedFile[], accepted: Accepted): ChangedFile[] {
  return files.filter((f) => accepted[f.path] !== f.fingerprint);
}

/** Atalhos da revisão: j/k arquivo seguinte e anterior, a aceitar, r reverter. */
export type ReviewKey = "next" | "prev" | "accept" | "revert";
const KEYS = new Map<string, ReviewKey>([["j", "next"], ["k", "prev"], ["a", "accept"], ["r", "revert"]]);
export function reviewKeyAction(key: string): ReviewKey | null {
  return KEYS.get(key.toLowerCase()) ?? null;
}

/** Caminho absoluto de um arquivo da missão (o caminho do Regente usa barras invertidas no Windows). */
export function absOf(root: string, rel: string): string {
  return root.replace(/[\\/]+$/, "") + "\\" + rel.replace(/\//g, "\\");
}

/** Mesmo arquivo no Windows: sem diferença de maiúsculas nem de separador. */
export function sameFile(a: string, b: string): boolean {
  const n = (p: string) => p.replace(/\//g, "\\").toLowerCase();
  return n(a) === n(b);
}

/** Texto do aviso antes de reverter um arquivo para a base. */
export function revertWarning(f: ChangedFile): string {
  return f.status === "A"
    ? `O arquivo novo será apagado.`
    : `As mudanças dele nesta missão serão desfeitas. Se já estiverem comitadas, a reversão vira um commit "revert".`;
}

/** Aviso da integração e do arquivamento quando o editor tem alterações não salvas no worktree. */
export const DIRTY_NOTE = "Há arquivos desta missão com alterações não salvas no editor; o que não foi salvo fica de fora.";

/** O editor tem alterações não salvas dentro do worktree da missão. */
export function dirtyIn(dirty: string[], worktreePath: string | null): boolean {
  const wt = worktreePath?.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return !!wt && dirty.some((p) => p.replace(/\//g, "\\").toLowerCase().startsWith(`${wt}\\`));
}

