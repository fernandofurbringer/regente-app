import { lazy, Suspense } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Components } from "react-markdown";
import { errMsg } from "./ipc";
import { useStore } from "../state/store";

const fail = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Links do markdown: http(s) abre no navegador do sistema; nada navega a janela. */
export const MD: Components = {
  a: ({ href, children }) => (
    <a href={href} onClick={(e) => { e.preventDefault(); if (href && /^https?:\/\//i.test(href)) openUrl(href).catch(fail); }}>{children}</a>
  ),
};

// react-markdown (e o parser inteiro) só entra quando há texto de agente para mostrar
const Body = lazy(() => import("./markdownBody"));

/** Texto escrito por agente (resumos de gate, memórias, etc.): links seguros e imagens nunca carregam (viram o texto alternativo). Enquanto o parser carrega, aparece o texto puro. */
export function AgentMarkdown({ children }: { children: string }) {
  return <Suspense fallback={<span style={{ whiteSpace: "pre-wrap" }}>{children}</span>}><Body>{children}</Body></Suspense>;
}
