import { describe, expect, it } from "vitest";
import { installSpec, needsPowerShell } from "./install";

describe("install", () => {
  it("comando do PowerShell (irm/iex/pipe) vai para o PowerShell", () => {
    const cmd = "irm 'https://cursor.com/install?win32=true' | iex";
    expect(needsPowerShell(cmd)).toBe(true);
    expect(installSpec("Cursor Agent", cmd)).toMatchObject({ kind: "terminal", name: "Instalar Cursor Agent", run: cmd, shell: "powershell" });
  });

  it("comando comum segue o shell padrão", () => {
    const cmd = "npm i -g @github/copilot";
    expect(needsPowerShell(cmd)).toBe(false);
    expect(installSpec("Copilot CLI", cmd).shell).toBeUndefined();
  });
});
