import { matches, parseHotkey, type KeyLike } from "./hotkey";

/** Atalhos da janela (não são globais do sistema). Só estes combos são capturados; o resto segue para o terminal. */
export interface ShortcutDef { id: string; label: string; combo: string }

export const SHORTCUTS: ShortcutDef[] = [
  { id: "paletteAlt", label: "Paleta de comandos", combo: "Ctrl+Shift+P" },
  { id: "palette", label: "Paleta de comandos (extra, fora do terminal)", combo: "Ctrl+K" },
  { id: "newTerminal", label: "Novo terminal", combo: "Ctrl+Shift+T" },
  { id: "newPanel", label: "Novo painel de IA (como o último)", combo: "Ctrl+Shift+N" },
  { id: "attention", label: "Ir para painel aguardando resposta", combo: "Ctrl+Shift+A" },
  { id: "newMission", label: "Nova missão", combo: "Ctrl+Shift+M" },
  { id: "gate", label: "Abrir gate pendente", combo: "Ctrl+Shift+G" },
  { id: "quickOpenAlt", label: "Abrir arquivo", combo: "Ctrl+Shift+O" },
  { id: "quickOpen", label: "Abrir arquivo (extra, fora do terminal)", combo: "Ctrl+P" },
  { id: "copyConversation", label: "Copiar conversa do painel ativo", combo: "Ctrl+Shift+C" },
  { id: "copyLastReply", label: "Copiar última resposta do agente", combo: "Ctrl+Shift+Y" },
  { id: "searchPanes", label: "Buscar em todos os painéis", combo: "Ctrl+Shift+F" },
  { id: "composer", label: "Compositor do painel de IA (mensagem com menções)", combo: "Ctrl+Shift+Enter" },
  { id: "activity", label: "Subtarefas e comandos do painel ativo", combo: "Ctrl+Shift+J" },
  { id: "gridMode", label: "Trocar o modo da grade", combo: "Ctrl+Shift+L" },
  { id: "nextPane", label: "Próximo painel", combo: "Ctrl+Tab" },
  { id: "prevPane", label: "Painel anterior", combo: "Ctrl+Shift+Tab" },
  ...Array.from({ length: 9 }, (_, i) => ({ id: `pane${i + 1}`, label: `Focar painel ${i + 1}`, combo: `Ctrl+${i + 1}` })),
];

export const DEFAULT_SHORTCUTS: Record<string, string> = Object.fromEntries(SHORTCUTS.map((s) => [s.id, s.combo]));

/** Padrões antigos que mudaram: um valor salvo igual a estes cai para o padrão novo (migração). */
const LEGACY: Record<string, string> = { newTerminal: "Ctrl+T" };

/** Padrões mais os atalhos salvos pelo usuário (ids desconhecidos e combos vazios são ignorados). */
export function resolveShortcuts(saved: Record<string, string> | undefined): Record<string, string> {
  const out = { ...DEFAULT_SHORTCUTS };
  for (const id of Object.keys(out)) {
    const combo = saved?.[id];
    if (combo && LEGACY[id] !== combo) out[id] = combo;
  }
  return out;
}

/** Dentro do terminal, Ctrl+letra vai para o programa (Ctrl+T do Claude Code, Ctrl+K do readline). Só Ctrl+Shift,
 *  Ctrl+Alt, Ctrl+dígito e Ctrl+Tab ficam com o Regente; fora do terminal o atalho sempre vale. */
export function interceptsIn(combo: string, inTerminal: boolean): boolean {
  if (!inTerminal) return true;
  const h = parseHotkey(combo);
  const plainCtrl = h.ctrl && !h.shift && !h.alt && !h.meta;
  if (!plainCtrl) return true;
  return /^Digit\d$/.test(h.code) || h.code === "Tab";
}

/** Ctrl+Shift+C deixa o copiar nativo agir quando há seleção (terminal ou página) ou o foco está num campo. */
export function copyDefers(s: { inputFocused: boolean; terminalSelection: boolean; pageSelection: string }): boolean {
  return s.inputFocused || s.terminalSelection || s.pageSelection.length > 0;
}

/** Forma canônica do combo ("Shift+Ctrl+M" e "Ctrl+Shift+M" são o mesmo). */
export function canonical(spec: string): string {
  const h = parseHotkey(spec);
  return [h.ctrl && "Ctrl", h.alt && "Alt", h.shift && "Shift", h.meta && "Win", h.code].filter(Boolean).join("+");
}

/** Combos que o terminal e o editor precisam; nunca viram atalho do Regente. */
const RESERVED = new Set(["Ctrl+C", "Ctrl+V", "Ctrl+X", "Ctrl+Z", "Ctrl+A", "Ctrl+D", "Ctrl+Shift+V"].map(canonical));

export interface GlobalCombo { label: string; combo: string }

/** Motivo para recusar `combo` como atalho de `id`, ou null se ele serve. `globals`: ditado e captura de tela. */
export function findConflict(id: string, combo: string, resolved: Record<string, string>, globals: GlobalCombo[]): string | null {
  const h = parseHotkey(combo);
  if (!h.code) return "Escolha uma tecla além dos modificadores.";
  if (!(h.ctrl || h.alt || h.meta) && !/^F\d+$/.test(h.code)) return "Use Ctrl, Alt ou Win na combinação.";
  const c = canonical(combo);
  if (RESERVED.has(c)) return `${combo} é do terminal (copiar, colar ou interromper): escolha outra.`;
  const other = Object.entries(resolved).find(([k, v]) => k !== id && canonical(v) === c);
  if (other) return `${combo} já é usado por "${SHORTCUTS.find((s) => s.id === other[0])?.label ?? other[0]}".`;
  const global = globals.find((g) => canonical(g.combo) === c);
  if (global) return `${combo} já é usado por ${global.label} (atalho global).`;
  return null;
}

/** Ação cujo combo bate com o evento, ou null. */
export function matchAction(e: KeyLike, resolved: Record<string, string>): string | null {
  return Object.keys(resolved).find((id) => matches(e, resolved[id])) ?? null;
}
