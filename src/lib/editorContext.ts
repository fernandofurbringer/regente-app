import type { editor } from "monaco-editor";

/** Estado do editor de cada workspace para as variáveis {{arquivo}} e {{selecao}}. O EditorArea registra e some ao desmontar. */
export interface EditorContext { path: string | null; selection: string }

const getters = new Map<string, () => EditorContext>();

export function registerEditor(wsId: string, get: () => EditorContext): () => void {
  getters.set(wsId, get);
  return () => {
    if (getters.get(wsId) === get) getters.delete(wsId);
  };
}

/** Arquivo aberto e seleção do editor do workspace; vazio se o editor não está montado. */
export function editorContext(wsId: string | null): EditorContext {
  const get = wsId ? getters.get(wsId) : undefined;
  if (!get) return { path: null, selection: "" };
  try {
    return get();
  } catch {
    return { path: null, selection: "" };
  }
}

const instancias = new Map<string, () => editor.IStandaloneCodeEditor | null>();

/** A barra de status age no editor do workspace (ir para a linha, fim de linha, indentação). O EditorArea registra e some
 *  ao desmontar. Só tipo do Monaco aqui: quem importa este arquivo não carrega o Monaco. */
export function registerEditorInstance(wsId: string, get: () => editor.IStandaloneCodeEditor | null): () => void {
  instancias.set(wsId, get);
  return () => {
    if (instancias.get(wsId) === get) instancias.delete(wsId);
  };
}

/** Instância do Monaco do workspace; null se o editor não está montado. */
export function editorInstance(wsId: string): editor.IStandaloneCodeEditor | null {
  return instancias.get(wsId)?.() ?? null;
}
