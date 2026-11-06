import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (p: string) => `asset://${p}` }));

import { isHttpsUrl, videoSrc } from "./media";
import { findLinks, isVideoPath } from "./links";

describe("videoSrc", () => {
  it("usa o asset protocol para arquivos locais", () => {
    expect(videoSrc("C:\\pessoal\\assets\\clip.mp4")).toBe("asset://C:\\pessoal\\assets\\clip.mp4");
    expect(videoSrc("/home/u/clip.mp4")).toBe("asset:///home/u/clip.mp4");
  });

  it("não toca em URL, UNC, marcação, javascript: nem caminho relativo", () => {
    expect(videoSrc("https://exemplo.com/a.mp4")).toBeNull();
    expect(videoSrc("//servidor/share/a.mp4")).toBeNull();
    expect(videoSrc("\\\\servidor\\share\\a.mp4")).toBeNull();
    expect(videoSrc("<img src=x onerror=alert(1)>")).toBeNull();
    expect(videoSrc("javascript:alert(1)")).toBeNull();
    expect(videoSrc("http://exemplo.com/a.mp4")).toBeNull();
    expect(videoSrc("assets/a.mp4")).toBeNull();
  });

  it("reconhece https para abrir no navegador", () => {
    expect(isHttpsUrl("https://exemplo.com/a.mp4")).toBe(true);
    expect(isHttpsUrl("http://exemplo.com/a.mp4")).toBe(false);
    expect(isHttpsUrl("C:\\a.mp4")).toBe(false);
  });
});

describe("links de vídeo no terminal", () => {
  it("reconhece mp4 e webm, com linha:coluna", () => {
    expect(isVideoPath("C:\\v\\clip.mp4:12")).toBe(true);
    expect(isVideoPath("clip.WEBM")).toBe(true);
    expect(isVideoPath("foto.png")).toBe(false);
  });

  it("um caminho de vídeo vira link de vídeo, não de arquivo", () => {
    const [found] = findLinks("gerado: C:\\pessoal\\assets\\clip-20261015-101010.mp4 pronto");
    expect(found).toMatchObject({ kind: "video", text: "C:\\pessoal\\assets\\clip-20261015-101010.mp4" });
  });
});
