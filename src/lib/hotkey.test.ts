import { describe, expect, it } from "vitest";
import { formatHotkey, matches, parseHotkey } from "./hotkey";

const ev = (code: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) => ({
  code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods,
});

describe("atalhos de teclado", () => {
  it("parseHotkey separa modificadores e tecla", () => {
    expect(parseHotkey("Ctrl+Space")).toEqual({ ctrl: true, alt: false, shift: false, meta: false, code: "Space" });
    expect(parseHotkey("Ctrl+Alt+Space")).toEqual({ ctrl: true, alt: true, shift: false, meta: false, code: "Space" });
    expect(parseHotkey("win+v").code).toBe("KeyV");
    expect(parseHotkey("F9").code).toBe("F9");
  });

  it("matches exige a tecla e exatamente os modificadores", () => {
    expect(matches(ev("Space", { ctrlKey: true }), "Ctrl+Space")).toBe(true);
    expect(matches(ev("Space"), "Ctrl+Space")).toBe(false);
    expect(matches(ev("Space", { ctrlKey: true, shiftKey: true }), "Ctrl+Space")).toBe(false);
    expect(matches(ev("KeyA", { ctrlKey: true }), "Ctrl+Space")).toBe(false);
    expect(matches(ev("Space", { ctrlKey: true }), "")).toBe(false);
  });

  it("formatHotkey grava o que foi apertado e fecha com matches", () => {
    const spec = formatHotkey(ev("KeyV", { ctrlKey: true, altKey: true }));
    expect(spec).toBe("Ctrl+Alt+V");
    expect(matches(ev("KeyV", { ctrlKey: true, altKey: true }), spec!)).toBe(true);
    expect(formatHotkey(ev("Digit1", { shiftKey: true }))).toBe("Shift+1");
  });

  it("formatHotkey ignora só modificadores", () => {
    expect(formatHotkey(ev("ControlLeft", { ctrlKey: true }))).toBeNull();
  });
});
