import { describe, expect, it } from "vitest";
import { editorTheme } from "./theme";

describe("tema do editor", () => {
  it("segue o tema do app; desconhecido fica no escuro", () => {
    expect(editorTheme("light")).toBe("vs");
    expect(editorTheme("contraste")).toBe("hc-black");
    expect(editorTheme("dark")).toBe("regente-aurora");
    expect(editorTheme(undefined)).toBe("regente-aurora");
  });
});
