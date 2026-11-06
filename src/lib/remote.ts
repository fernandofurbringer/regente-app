/** Workspaces remotos (`ssh://alias/caminho`). A validação espelha src-tauri/src/remote.rs; o backend valida de novo. */
export const isRemote = (p: string) => p.startsWith("ssh://");

const ALIAS = /^[A-Za-z0-9._@][A-Za-z0-9._@-]*$/;
const UNSAFE = /[\x00-\x1f\x7f'"`\\]/;

/** Nome de sessão tmux que pode ir ao servidor (mesma regra de `safe_text` no Rust). */
export const safeSessionName = (s: string) => s.length > 0 && s.length <= 128 && !UNSAFE.test(s);

export function remoteTarget(alias: string, path: string): { url: string } | { error: string } {
  const a = alias.trim();
  const raw = path.trim();
  if (raw === "/") return { error: "Escolha uma pasta, não a raiz do servidor." };
  const p = raw.replace(/\/+$/, "");
  if (!a || a.length > 255 || !ALIAS.test(a)) return { error: "Escolha um alias do ~/.ssh/config (letras, números, . - _ @)." };
  if (!p.startsWith("/")) return { error: "Use um caminho absoluto no servidor, começando por /." };
  if (UNSAFE.test(p) || p.split("/").includes("..")) return { error: "Caminho inválido: sem aspas, barras invertidas nem '..'." };
  return { url: `ssh://${a}${p}` };
}
