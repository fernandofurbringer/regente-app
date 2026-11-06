/** Atalhos como texto ("Ctrl+Alt+Space") comparados pelo `KeyboardEvent.code`, que não depende do layout. */

export interface Hotkey { ctrl: boolean; alt: boolean; shift: boolean; meta: boolean; code: string }
export interface KeyLike { code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean }

const MODIFIERS: Record<string, "ctrl" | "alt" | "shift" | "meta"> = {
  ctrl: "ctrl", control: "ctrl", alt: "alt", shift: "shift", win: "meta", meta: "meta", super: "meta",
};
const NAMED: Record<string, string> = {
  space: "Space", espaço: "Space", enter: "Enter", tab: "Tab", esc: "Escape", escape: "Escape", backspace: "Backspace", "`": "Backquote",
};
const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"]);

function keyCode(k: string): string {
  if (/^[a-z]$/i.test(k)) return `Key${k.toUpperCase()}`;
  if (/^[0-9]$/.test(k)) return `Digit${k}`;
  if (/^f([1-9]|1[0-2])$/i.test(k)) return k.toUpperCase();
  return NAMED[k.toLowerCase()] ?? k;
}

export function parseHotkey(spec: string): Hotkey {
  const h: Hotkey = { ctrl: false, alt: false, shift: false, meta: false, code: "" };
  for (const part of spec.split("+").map((s) => s.trim()).filter(Boolean)) {
    const mod = MODIFIERS[part.toLowerCase()];
    if (mod) h[mod] = true;
    else h.code = keyCode(part);
  }
  return h;
}

/** True when the event is exactly the hotkey: same key and the same modifiers pressed (no extra ones). */
export function matches(e: KeyLike, spec: string): boolean {
  const h = parseHotkey(spec);
  return h.code !== "" && e.code === h.code && e.ctrlKey === h.ctrl && e.altKey === h.alt && e.shiftKey === h.shift && e.metaKey === h.meta;
}

/** Text for a keydown, e.g. Ctrl + KeyV → "Ctrl+V". Null while only modifiers are held. */
export function formatHotkey(e: KeyLike): string | null {
  if (MODIFIER_CODES.has(e.code) || !e.code) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Win");
  parts.push(e.code.replace(/^Key/, "").replace(/^Digit/, ""));
  return parts.join("+");
}
