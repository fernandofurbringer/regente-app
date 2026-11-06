import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * Fonte do `<video>`: só arquivo local (asset protocol), por caminho de unidade (C:\...) ou caminho absoluto Unix.
 * Qualquer outra coisa fica de fora: UNC (\\servidor e //servidor), https, javascript:, HTML, caminho relativo.
 * URL https não toca no painel: abre no navegador (ver `isHttpsUrl`).
 */
export function videoSrc(p: string): string | null {
  const t = p.trim();
  if (t.startsWith("//") || t.startsWith("\\\\")) return null;
  if (/^[A-Za-z]:[\\/]/.test(t) || (t.startsWith("/") && !t.startsWith("//"))) return convertFileSrc(t);
  return null;
}

export function isHttpsUrl(p: string): boolean {
  return /^https:\/\//i.test(p.trim());
}

/** Extensões que abrem no painel de vídeo quando aparecem no terminal. */
export const VIDEO_RE = /\.(mp4|webm)$/i;
