import { describe, expect, it } from "vitest";
import { formatHotkey } from "./hotkey";
import { copyDefers, DEFAULT_SHORTCUTS, canonical, findConflict, interceptsIn, matchAction, resolveShortcuts, SHORTCUTS } from "./shortcuts";

const ev = (code: string, mods: Partial<Record<"ctrlKey" | "altKey" | "shiftKey" | "metaKey", boolean>> = {}) => ({
  code, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...mods,
});
const GLOBALS = [{ label: "o ditado global", combo: "Ctrl+Alt+Space" }, { label: "a captura de tela", combo: "Ctrl+Shift+5" }];

describe("atalhos da janela", () => {
  it("padrões não se chocam entre si nem com os globais", () => {
    const resolved = resolveShortcuts(undefined);
    for (const s of SHORTCUTS) {
      expect(findConflict(s.id, s.combo, resolved, GLOBALS), s.id).toBeNull();
    }
  });

  it("resolveShortcuts mistura os salvos com os padrões e ignora ids e vazios desconhecidos", () => {
    const r = resolveShortcuts({ palette: "Ctrl+Alt+K", newTerminal: "", fantasma: "Ctrl+Q" });
    expect(r.palette).toBe("Ctrl+Alt+K");
    expect(r.newTerminal).toBe(DEFAULT_SHORTCUTS.newTerminal);
    expect(r).not.toHaveProperty("fantasma");
  });

  it("canonical ignora a ordem dos modificadores", () => {
    expect(canonical("Shift+Ctrl+M")).toBe(canonical("Ctrl+Shift+M"));
    expect(canonical("Ctrl+Alt+Space")).toBe("Ctrl+Alt+Space");
  });

  it("recusa Ctrl+C e Ctrl+V, e teclas soltas sem modificador", () => {
    const r = resolveShortcuts(undefined);
    expect(findConflict("palette", "Ctrl+C", r, [])).toMatch(/terminal/);
    expect(findConflict("palette", "Ctrl+V", r, [])).toMatch(/terminal/);
    expect(findConflict("palette", "Ctrl+Shift+V", r, [])).toMatch(/terminal/);
    expect(findConflict("palette", "K", r, [])).toMatch(/Ctrl, Alt ou Win/);
    expect(findConflict("palette", "Ctrl+", r, [])).toMatch(/tecla/);
    // tecla de função sozinha é aceita
    expect(findConflict("palette", "F8", r, [])).toBeNull();
  });

  it("recusa combo de outro atalho do app, inclusive em outra ordem de modificadores", () => {
    const r = resolveShortcuts(undefined);
    expect(findConflict("palette", "Shift+Ctrl+M", r, [])).toMatch(/Nova missão/);
    // o próprio atalho pode ser regravado com o mesmo combo
    expect(findConflict("newMission", "Ctrl+Shift+M", r, [])).toBeNull();
  });

  it("recusa combo igual a um atalho global (ditado ou captura)", () => {
    const r = resolveShortcuts(undefined);
    expect(findConflict("palette", "Ctrl+Alt+Space", r, GLOBALS)).toMatch(/ditado global/);
    expect(findConflict("palette", "Ctrl+Shift+5", r, GLOBALS)).toMatch(/captura de tela/);
  });

  it("matchAction acha a ação do evento e ignora o que não é atalho", () => {
    const r = resolveShortcuts(undefined);
    expect(matchAction(ev("KeyK", { ctrlKey: true }), r)).toBe("palette");
    expect(matchAction(ev("Digit3", { ctrlKey: true }), r)).toBe("pane3");
    expect(matchAction(ev("Tab", { ctrlKey: true, shiftKey: true }), r)).toBe("prevPane");
    expect(matchAction(ev("KeyK", { ctrlKey: true, altKey: true }), r)).toBeNull();
    expect(matchAction(ev("KeyC", { ctrlKey: true }), r)).toBeNull();
    expect(formatHotkey(ev("Tab", { ctrlKey: true }))).toBe("Ctrl+Tab");
  });
});

describe("foco no terminal e cópia", () => {
  it("dentro do terminal, Ctrl+letra vai para o programa; Ctrl+Shift, Ctrl+Alt, Ctrl+dígito e Ctrl+Tab ficam com o Regente", () => {
    expect(interceptsIn("Ctrl+T", true)).toBe(false);
    expect(interceptsIn("Ctrl+K", true)).toBe(false);
    expect(interceptsIn("Ctrl+P", true)).toBe(false);
    expect(interceptsIn("Ctrl+Shift+P", true)).toBe(true);
    expect(interceptsIn("Ctrl+Alt+K", true)).toBe(true);
    expect(interceptsIn("Ctrl+3", true)).toBe(true);
    expect(interceptsIn("Ctrl+Tab", true)).toBe(true);
    expect(interceptsIn("Ctrl+Shift+Tab", true)).toBe(true);
  });

  it("fora do terminal, todo atalho vale", () => {
    expect(interceptsIn("Ctrl+K", false)).toBe(true);
    expect(interceptsIn("Ctrl+T", false)).toBe(true);
  });

  it("Ctrl+Shift+C não age quando há seleção ou foco num campo", () => {
    const none = { inputFocused: false, terminalSelection: false, pageSelection: "" };
    expect(copyDefers(none)).toBe(false);
    expect(copyDefers({ ...none, terminalSelection: true })).toBe(true);
    expect(copyDefers({ ...none, pageSelection: "trecho" })).toBe(true);
    expect(copyDefers({ ...none, inputFocused: true })).toBe(true);
  });

  it("novos padrões: nova aba Ctrl+Shift+T, abrir arquivo Ctrl+Shift+O e paleta alternativa Ctrl+Shift+P", () => {
    const r = resolveShortcuts(undefined);
    expect(r.newTerminal).toBe("Ctrl+Shift+T");
    expect(r.quickOpenAlt).toBe("Ctrl+Shift+O");
    expect(r.paletteAlt).toBe("Ctrl+Shift+P");
    expect(matchAction({ code: "KeyP", ctrlKey: true, altKey: false, shiftKey: true, metaKey: false }, r)).toBe("paletteAlt");
  });

  it("migra o Ctrl+T antigo salvo para o padrão novo, e mantém os outros atalhos salvos", () => {
    expect(resolveShortcuts({ newTerminal: "Ctrl+T" }).newTerminal).toBe("Ctrl+Shift+T");
    expect(resolveShortcuts({ newTerminal: "Ctrl+Alt+N" }).newTerminal).toBe("Ctrl+Alt+N");
  });
});

describe("novos atalhos de painel (B-03, B-04)", () => {
  it("Ctrl+Shift+N é novo painel de IA e Ctrl+Shift+A vai ao painel que aguarda resposta, sem colidir", () => {
    const r = resolveShortcuts(undefined);
    expect(r.newPanel).toBe("Ctrl+Shift+N");
    expect(r.attention).toBe("Ctrl+Shift+A");
    expect(matchAction(ev("KeyN", { ctrlKey: true, shiftKey: true }), r)).toBe("newPanel");
    expect(matchAction(ev("KeyA", { ctrlKey: true, shiftKey: true }), r)).toBe("attention");
    expect(findConflict("newPanel", "Ctrl+Shift+A", r, GLOBALS)).toMatch(/Ir para painel/);
  });

  it("os dois são Ctrl+Shift, então o terminal os deixa com o Regente", () => {
    expect(interceptsIn("Ctrl+Shift+N", true)).toBe(true);
    expect(interceptsIn("Ctrl+Shift+A", true)).toBe(true);
  });
});

describe("copiar a última resposta (Ctrl+Shift+Y)", () => {
  it("é um padrão livre: não colide com os outros atalhos nem com os globais", () => {
    const resolved = resolveShortcuts(undefined);
    expect(resolved.copyLastReply).toBe("Ctrl+Shift+Y");
    expect(findConflict("copyLastReply", "Ctrl+Shift+Y", resolved, GLOBALS)).toBeNull();
    expect(matchAction(ev("KeyY", { ctrlKey: true, shiftKey: true }), resolved)).toBe("copyLastReply");
  });
});
