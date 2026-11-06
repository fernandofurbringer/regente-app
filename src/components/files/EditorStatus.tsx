import { useCallback, useState } from "react";
import { editorInstance } from "../../lib/editorContext";
import { indentTexto, posTexto } from "../../lib/editorStatus";
import { fmtSessionSize } from "../../lib/sessions";
import { useStore } from "../../state/store";
import FloatMenu from "../FloatMenu";

/** ações do Monaco no menu de indentação (as mesmas do VS Code) */
const INDENT: [string, string][] = [
  ["editor.action.indentUsingSpaces", "Indentar com espaços…"],
  ["editor.action.indentUsingTabs", "Indentar com tabulação…"],
  ["editor.action.indentationToSpaces", "Converter a indentação para espaços"],
  ["editor.action.indentationToTabs", "Converter a indentação para tabulação"],
  ["editor.action.detectIndentation", "Detectar pela indentação do arquivo"],
];

/** Lado esquerdo da barra de status na visão de arquivos, como no VS Code: posição (clique = ir para a linha), indentação
 *  (clique = menu), codificação, fim de linha (clique troca CRLF/LF), linguagem, tamanho em disco e "modificado".
 *  Não importa o Monaco (fica fora do pacote principal): age pela instância registrada pelo EditorArea. */
export default function EditorStatus({ wsId }: { wsId: string }) {
  const st = useStore((s) => s.editorStatus[wsId] ?? null);
  const [menu, setMenu] = useState<DOMRect | null>(null);
  const fechar = useCallback(() => setMenu(null), []);
  if (!st) return null;
  const rodar = (id: string) => {
    setMenu(null);
    const ed = editorInstance(wsId);
    ed?.focus();
    void ed?.getAction(id)?.run();
  };
  // EndOfLineSequence do Monaco: LF = 0, CRLF = 1; pushEOL entra no desfazer e marca o arquivo como modificado
  const trocarEol = () => editorInstance(wsId)?.getModel()?.pushEOL(st.eol === "CRLF" ? 0 : 1);
  return (
    <div className="edstatus" aria-label="Estado do editor">
      <button className="edstatus-item" title="Ir para a linha (Ctrl+G)" onClick={() => rodar("editor.action.gotoLine")}>{posTexto(st)}</button>
      <button className="edstatus-item" data-menu-trigger="" aria-haspopup="menu" aria-expanded={!!menu} disabled={st.readOnly} title="Indentação"
        onClick={(e) => setMenu(menu ? null : e.currentTarget.getBoundingClientRect())}>{indentTexto(st)}</button>
      <span className="edstatus-item" title="Codificação do arquivo">UTF-8</span>
      <button className="edstatus-item" disabled={st.readOnly} title={`Fim de linha: clique para trocar para ${st.eol === "CRLF" ? "LF" : "CRLF"}`} onClick={trocarEol}>{st.eol}</button>
      <span className="edstatus-item" title="Linguagem">{st.lang}</span>
      <span className="edstatus-item" title="Tamanho no disco">{fmtSessionSize(st.bytes)}</span>
      {st.readOnly ? <span className="edstatus-item">somente leitura</span> : st.dirty && <span className="edstatus-item edstatus-sujo">● modificado</span>}
      {menu && (
        <FloatMenu at={menu} onClose={fechar} label="Indentação">
          {INDENT.map(([id, rotulo]) => <button key={id} className="menu-item" role="menuitem" onClick={() => rodar(id)}>{rotulo}</button>)}
        </FloatMenu>
      )}
    </div>
  );
}
