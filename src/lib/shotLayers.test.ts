import { describe, expect, it } from "vitest";
import { addLayer, applyCrop, newHistory, parseDoc, redo, setCrop, toImageRect, undo, type Layer } from "./shotLayers";

const rect: Layer = { type: "rect", rect: { x: 10, y: 10, w: 20, h: 20 }, color: "red" };
const arrow: Layer = { type: "arrow", from: { x: 50, y: 50 }, to: { x: 60, y: 60 }, color: "red" };

describe("history", () => {
  it("adds layers and undoes/redoes them", () => {
    let h = newHistory();
    h = addLayer(h, rect);
    h = addLayer(h, arrow);
    expect(h.present.layers).toHaveLength(2);
    h = undo(h);
    expect(h.present.layers).toEqual([rect]);
    h = undo(h);
    expect(h.present.layers).toEqual([]);
    h = redo(h);
    expect(h.present.layers).toEqual([rect]);
    h = redo(redo(h));
    expect(h.present.layers).toEqual([rect, arrow]);
  });

  it("a new edit after an undo drops the redo stack", () => {
    let h = addLayer(newHistory(), rect);
    h = undo(h);
    h = addLayer(h, arrow);
    expect(h.future).toEqual([]);
    expect(redo(h)).toBe(h);
  });

  it("undo and redo do nothing at the ends", () => {
    const h = newHistory();
    expect(undo(h)).toBe(h);
    expect(redo(h)).toBe(h);
  });

  it("crop is a document property with its own undo", () => {
    let h = setCrop(newHistory(), { x: 5, y: 5, w: 100, h: 50 });
    expect(h.present.crop).toEqual({ x: 5, y: 5, w: 100, h: 50 });
    h = undo(h);
    expect(h.present.crop).toBeNull();
  });
});

describe("applyCrop", () => {
  it("moves layers into the crop origin and keeps the ones that touch it", () => {
    const moved = applyCrop([rect, arrow], { x: 40, y: 40, w: 100, h: 100 });
    expect(moved).toEqual([{ ...arrow, from: { x: 10, y: 10 }, to: { x: 20, y: 20 } }]);
  });

  it("drops a layer that lies fully outside the crop", () => {
    expect(applyCrop([rect], { x: 100, y: 100, w: 10, h: 10 })).toEqual([]);
  });
});

describe("toImageRect", () => {
  it("maps a CSS selection to image pixels (HiDPI) and clamps it to the image", () => {
    // 1920x1200 image on a 960x600 window (scale 2): a 100x50 CSS box at (10,20)
    expect(toImageRect({ x: 10, y: 20, w: 100, h: 50 }, 960, 600, 1920, 1200)).toEqual({ x: 20, y: 40, w: 200, h: 100 });
    expect(toImageRect({ x: 900, y: 580, w: 200, h: 200 }, 960, 600, 1920, 1200)).toEqual({ x: 1800, y: 1160, w: 120, h: 40 });
  });
});

describe("parseDoc", () => {
  it("reads a saved document and falls back to an empty one for garbage", () => {
    const text = JSON.stringify({ crop: { x: 1, y: 2, w: 3, h: 4 }, layers: [rect] });
    expect(parseDoc(text)).toEqual({ crop: { x: 1, y: 2, w: 3, h: 4 }, layers: [rect] });
    expect(parseDoc("{nao e json")).toEqual({ crop: null, layers: [] });
    expect(parseDoc(null)).toEqual({ crop: null, layers: [] });
    expect(parseDoc(JSON.stringify({ crop: "x", layers: 3 }))).toEqual({ crop: null, layers: [] });
  });
});
