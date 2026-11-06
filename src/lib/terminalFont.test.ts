import { describe, expect, it } from "vitest";
import rustSettings from "../../src-tauri/src/settings.rs?raw";
import { TERMINAL_FONT_DEFAULT, firstFontFamily } from "./terminalFont";

describe("família da fonte do terminal", () => {
  it("pega a primeira família, com ou sem aspas", () => {
    expect(firstFontFamily("'FiraCode Nerd Font', 'Hack Nerd Font', monospace")).toBe("FiraCode Nerd Font");
    expect(firstFontFamily('"JetBrains Mono", monospace')).toBe("JetBrains Mono");
    expect(firstFontFamily("Consolas, monospace")).toBe("Consolas");
    expect(firstFontFamily("'Vírgula, Font', monospace")).toBe("Vírgula, Font");
    expect(firstFontFamily("  ")).toBe("monospace");
  });

  it("o padrão do TS é o mesmo do Rust (settings.rs)", () => {
    const m = /TERMINAL_FONT_DEFAULT: &str = "(.*?)";/.exec(rustSettings);
    expect(m?.[1]).toBe(TERMINAL_FONT_DEFAULT);
  });
});
