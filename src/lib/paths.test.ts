import { describe, expect, it } from "vitest";
import { formatDroppedPaths, quoted } from "./paths";

describe("quoted", () => {
  it("wraps a path that has a space", () => {
    expect(quoted("C:\\Users\\Ana Maria\\Pictures\\Regente Capturas\\shot-1.png")).toBe('"C:\\Users\\Ana Maria\\Pictures\\Regente Capturas\\shot-1.png"');
  });
  it("leaves a plain path alone", () => {
    expect(quoted("C:\\Users\\ana\\shot-1.png")).toBe("C:\\Users\\ana\\shot-1.png");
  });
});

describe("formatDroppedPaths", () => {
  it("quotes only the paths with spaces and joins them with one space, no Enter", () => {
    expect(formatDroppedPaths(["C:\a\b.txt", "C:\Meus Docs\c.png"])).toBe('C:\a\b.txt "C:\Meus Docs\c.png"');
  });
  it("is empty for no paths", () => {
    expect(formatDroppedPaths([])).toBe("");
  });
});
