import { describe, expect, it } from "vitest";
import { dropTargetAt, pastThreshold } from "./pointerDrag";

type Node = { dropped: boolean; parent?: Node };
/** Um elemento falso: `closest("[data-drop]")` sobe a cadeia de pais, como o DOM. */
const fake = (n: Node) => ({
  closest: (sel: string) => {
    for (let c: Node | undefined = n; c; c = c.parent) if (sel === "[data-drop]" && c.dropped) return c as unknown as Element;
    return null;
  },
});

describe("pastThreshold", () => {
  it("ignores movement under 5 px (a click)", () => {
    expect(pastThreshold(3, 3)).toBe(false);
    expect(pastThreshold(0, 0)).toBe(false);
  });
  it("starts at 5 px", () => {
    expect(pastThreshold(0, 5)).toBe(true);
    expect(pastThreshold(-4, -4)).toBe(true);
  });
});

describe("dropTargetAt", () => {
  it("resolves the innermost [data-drop] under the pointer", () => {
    const zone: Node = { dropped: true };
    const text: Node = { dropped: false, parent: zone };
    let at: [number, number] | null = null;
    const hit = dropTargetAt(10, 20, (x, y) => { at = [x, y]; return fake(text); });
    expect(at).toEqual([10, 20]);
    expect(hit).toBe(zone as unknown as Element);
  });
  it("returns null when no zone is under the pointer", () => {
    expect(dropTargetAt(1, 1, () => fake({ dropped: false }))).toBeNull();
    expect(dropTargetAt(1, 1, () => null)).toBeNull();
  });
});
