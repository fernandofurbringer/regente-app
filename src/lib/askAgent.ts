import { editorContext } from "./editorContext";
import { capText, SEND_MAX } from "./sendToAgent";
import { useStore, wsOf, type SendTo } from "../state/store";

export type AgentVerb = "explicar" | "revisar" | "testes" | "refatorar";
export const VERB_ORDER: AgentVerb[] = ["explicar", "revisar", "testes", "refatorar"];

export function verbLabel(v: AgentVerb): string {
  return VERBS[v].label;
}

const VERBS: Record<AgentVerb, { label: string; pede: (alvo: string) => string }> = {
  explicar: { label: "explicar", pede: (a) => `Explique o que ${a} faz.` },
  revisar: { label: "revisar", pede: (a) => `Revise ${a} e aponte problemas.` },
  testes: { label: "escrever testes", pede: (a) => `Escreva testes para ${a}.` },
  refatorar: { label: "refatorar", pede: (a) => `Refatore ${a} sem mudar o comportamento.` },
};

/** Caminho relativo à pasta do workspace (o agente trabalha dentro dela). */
export function relPath(path: string, root: string): string {
  const r = root.replace(/[\/]+$/, "");
  return path.startsWith(r) && r ? path.slice(r.length).replace(/^[\/]+/, "") || path : path;
}

/** Pasta de cima. Barra no fim (o git lista assim repositório aninhado: `?? wt/`) não impede de subir. */
export const parentOf = (p: string) => p.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]+$/, "");

/** Pedido curto ao agente sobre o arquivo (ou a seleção dele), em pt-BR. A seleção vai junto, cortada. */
export function askAboutFile(verb: AgentVerb, c: { workspaceId: string; missionId: string | null; path: string; root: string; selection: string }): SendTo {
  const rel = relPath(c.path, c.root);
  const sel = c.selection.trim();
  const alvo = sel ? `esta seleção de \`${rel}\`` : `o arquivo \`${rel}\``;
  const code = sel ? `\n\n\`\`\`\n${capText(c.selection, SEND_MAX)}\n\`\`\`` : "";
  return { workspaceId: c.workspaceId, missionId: c.missionId, text: VERBS[verb].pede(alvo) + code, title: `Pedir ao agente: ${VERBS[verb].label}` };
}

/** Abre o "Enviar ao agente" com o pedido sobre o arquivo (ou seleção) aberto no editor do workspace ativo. */
export function askFromEditor(verb: AgentVerb): string | null {
  const st = useStore.getState();
  const ws = wsOf(st.active);
  const ctx = editorContext(st.active);
  if (!ws || !ctx.path) return "Abra um arquivo no editor primeiro.";
  st.patch({ sendTo: askAboutFile(verb, { workspaceId: ws.id, missionId: st.missionSel[ws.id] ?? null, path: ctx.path, root: ws.path, selection: ctx.selection }) });
  return null;
}

/** "Enviar seleção ao agente": o texto selecionado no terminal vai ao diálogo de envio, cortado. */
export function selectionSendTo(c: { workspaceId: string; missionId: string | null }, selection: string): SendTo {
  return { workspaceId: c.workspaceId, missionId: c.missionId, title: "Enviar seleção ao agente", text: `Sobre isto:\n\`\`\`\n${capText(selection)}\n\`\`\`` };
}
