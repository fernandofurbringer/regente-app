import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errMsg, type PaneStatus, type PromptDef } from "../lib/ipc";
import { editorContext } from "../lib/editorContext";
import { expandPrompt, promptVarsUsed, type PromptValues } from "../lib/promptVars";
import { useStore, wsOf } from "../state/store";
import Modal from "./Modal";

const WRITABLE = ["terminal", "agent"];
const RUNNING: PaneStatus[] = ["iniciando", "trabalhando", "ocioso", "pergunta"];

/** Expande as variáveis e cola o prompt no painel (bracketed paste; Enter só se o prompt pedir). false = não colou. */
export async function insertPrompt(p: PromptDef, paneId: string): Promise<boolean> {
  const st = useStore.getState();
  const pane = st.panes[paneId];
  if (!pane || !WRITABLE.includes(pane.kind)) {
    st.toast("Escolha um painel de terminal ou IA para colar o prompt.", "erro");
    return false;
  }
  if (!RUNNING.includes(pane.status)) {
    st.toast(`${pane.name} não está rodando`, "erro");
    return false;
  }
  const used = promptVarsUsed(p.text);
  const ctx = editorContext(pane.workspaceId);
  const values: PromptValues = {};
  if (used.includes("clipboard")) values.clipboard = await navigator.clipboard.readText().catch(() => "");
  if (used.includes("arquivo")) values.arquivo = ctx.path ?? "";
  if (used.includes("selecao")) values.selecao = ctx.selection;
  if (used.includes("workspace")) values.workspace = wsOf(pane.workspaceId)?.path ?? "";
  if (used.includes("branch")) values.branch = await api.workspaceBranch(pane.workspaceId).catch(() => "");
  const empty = used.filter((v) => !values[v]);
  if (empty.length) {
    st.toast(`"${p.name}": sem valor para ${empty.map((v) => `{{${v}}}`).join(", ")}.`, "erro");
    return false;
  }
  // terminal nunca recebe Enter: o comando fica na linha para o usuário conferir
  const enter = p.sendEnter && pane.kind !== "terminal";
  // conteúdo da área de transferência seguido de Enter: a pessoa vê um trecho antes de enviar
  if (enter && used.includes("clipboard")) {
    const clip = values.clipboard ?? "";
    const preview = clip.slice(0, 200) + (clip.length > 200 ? "…" : "");
    if (!(await ask(`Colar a área de transferência e enviar Enter em ${pane.name}?

${preview}`, { title: "Enviar prompt", kind: "warning" }))) return false;
  }
  try {
    await api.promptPaste(pane.id, expandPrompt(p.text, values), enter);
    return true;
  } catch (e) {
    st.toast(errMsg(e), "erro");
    return false;
  }
}

/** Paleta: cola no painel ativo do workspace aberto. */
export function insertIntoActive(p: PromptDef) {
  const st = useStore.getState();
  const pane = st.panes[st.activePane ?? ""];
  if (!pane || pane.workspaceId !== st.active) return st.toast("Clique num painel para escolhê-lo primeiro.", "erro");
  void insertPrompt(p, pane.id);
}

/** Lista de prompts salvos para colar no painel indicado (menu ⋯ do painel). */
export default function PromptPicker({ paneId, onClose }: { paneId: string; onClose(): void }) {
  const [prompts, setPrompts] = useState<PromptDef[] | null>(null);
  useEffect(() => {
    api.promptsList().then(setPrompts).catch((e) => { useStore.getState().toast(errMsg(e), "erro"); setPrompts([]); });
  }, []);
  return (
    <Modal label="Prompts salvos" onClose={onClose} style={{ width: "min(560px, 92vw)" }}>
      <h2>Prompts salvos</h2>
      {prompts === null && <div className="meta">Carregando…</div>}
      {prompts?.length === 0 && <div className="meta">Nenhum prompt. Crie em Configurações → Prompts.</div>}
      <div role="list" style={{ display: "flex", flexDirection: "column", gap: 6, maxHeight: "55vh", overflowY: "auto" }}>
        {prompts?.map((p) => (
          <button key={p.id} className="menu-item" role="listitem" style={{ display: "flex", flexDirection: "column", alignItems: "flex-start" }}
            onClick={async () => { if (await insertPrompt(p, paneId)) onClose(); }}>
            <span>{p.name}{p.sendEnter ? " · envia Enter" : ""}</span>
            <span className="meta" style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis", maxWidth: "100%" }}>{p.text.split("\n")[0]}</span>
          </button>
        ))}
      </div>
    </Modal>
  );
}
