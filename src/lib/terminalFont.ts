/** Stack padrão do terminal. Espelho de `TERMINAL_FONT_DEFAULT` em `src-tauri/src/settings.rs` (teste em terminalFont.test.ts). */
export const TERMINAL_FONT_DEFAULT =
  "'FiraCode Nerd Font', 'FiraCode Nerd Font Mono', 'CaskaydiaCove Nerd Font', 'CaskaydiaMono Nerd Font', 'JetBrainsMono Nerd Font', 'MesloLGS NF', 'Hack Nerd Font', 'Symbols Nerd Font Mono', 'Cascadia Mono', Consolas, monospace";

/** Primeira família de uma stack CSS: `'FiraCode Nerd Font', monospace` -> `FiraCode Nerd Font`. */
export function firstFontFamily(stack: string): string {
  const quoted = /^\s*(['"])(.*?)\1/.exec(stack);
  if (quoted) return quoted[2];
  return stack.split(",")[0].trim() || "monospace";
}

/** Espera a fonte carregar (fontes web) antes de o xterm medir a célula; no máximo `timeoutMs`. */
export function loadTerminalFont(stack: string, size: number, timeoutMs = 1500): Promise<void> {
  const load = document.fonts.load(`${size}px "${firstFontFamily(stack)}"`).then(() => {}, () => {});
  let timer: number | undefined;
  const timeout = new Promise<void>((resolve) => {
    timer = window.setTimeout(resolve, timeoutMs);
  });
  return Promise.race([load, timeout]).finally(() => clearTimeout(timer));
}
