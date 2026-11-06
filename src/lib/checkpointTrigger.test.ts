import { describe, expect, it } from "vitest";
import { scopeOf } from "./checkpointTrigger";
import type { Mission, PaneRow } from "./ipc";

const pane = (missionId: string | null) => ({ id: "p1", workspaceId: "ws1", missionId, kind: "agent", name: "Claude" }) as unknown as PaneRow;
const mission = (id: string, worktreePath: string | null) => ({ id, workspaceId: "ws1", worktreePath }) as unknown as Mission;

describe("scopeOf", () => {
  it("a missão com worktree é o escopo", () => {
    expect(scopeOf(pane("m1"), [mission("m1", "C:/wt/m1")], false)).toBe("m1");
  });

  it("missão sem worktree cai no workspace só com a raiz ligada", () => {
    const ms = [mission("m1", null)];
    expect(scopeOf(pane("m1"), ms, false)).toBeNull();
    expect(scopeOf(pane("m1"), ms, true)).toBe("ws1");
  });

  it("painel sem missão segue a mesma regra da raiz", () => {
    expect(scopeOf(pane(null), [], false)).toBeNull();
    expect(scopeOf(pane(null), [], true)).toBe("ws1");
  });

  it("workspace remoto (ssh://) não tem checkpoint automático", () => {
    const remoto = { ...pane(null), cwd: "ssh://srv/home/u/proj" } as unknown as PaneRow;
    expect(scopeOf(remoto, [], true)).toBeNull();
  });

  it("workspace em modo sensível não tem checkpoint automático", () => {
    expect(scopeOf(pane(null), [], true, ["ws1"])).toBeNull();
    expect(scopeOf(pane(null), [], true, ["outro"])).toBe("ws1");
  });
});
