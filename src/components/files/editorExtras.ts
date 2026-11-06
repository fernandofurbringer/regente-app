import { monaco } from "../../monaco";
import { api } from "../../lib/ipc";
import { diffLinhas } from "../../lib/diffLinhas";
import { faixasIndentacao } from "../../lib/indentRainbow";

type Ed = monaco.editor.IStandaloneCodeEditor;
/** Linhas além das visíveis que já ganham faixa (rolagem curta sem piscar). */
const FOLGA = 20;

/** Indent-rainbow: faixas coloridas por nível nas linhas visíveis; repinta ao rolar e ao editar. Devolve o desligar. */
export function arcoIrisIndentacao(ed: Ed): () => void {
  const decs = ed.createDecorationsCollection();
  let quadro = 0;
  const pintar = () => {
    quadro = 0;
    const model = ed.getModel();
    if (!model) return decs.clear();
    const o = model.getOptions();
    const tab = o.indentSize || o.tabSize;
    const novas: monaco.editor.IModelDeltaDecoration[] = [];
    for (const r of ed.getVisibleRanges()) {
      const fim = Math.min(model.getLineCount(), r.endLineNumber + FOLGA);
      for (let ln = Math.max(1, r.startLineNumber - FOLGA); ln <= fim; ln++) {
        for (const f of faixasIndentacao(model.getLineContent(ln), tab)) {
          novas.push({ range: new monaco.Range(ln, f.inicio, ln, f.fim), options: { inlineClassName: `ir ir-${f.nivel}` } });
        }
      }
    }
    decs.set(novas);
  };
  const agendar = () => { if (!quadro) quadro = requestAnimationFrame(pintar); };
  const subs = [ed.onDidScrollChange(agendar), ed.onDidChangeModelContent(agendar), ed.onDidChangeModel(agendar)];
  agendar();
  return () => { subs.forEach((s) => s.dispose()); cancelAnimationFrame(quadro); decs.clear(); };
}

/** Margem do Git: barras verdes (nova), azuis (alterada) e triângulo vermelho (removida) contra o último commit.
 * A base é relida ao trocar de arquivo, ao salvar e quando a janela volta ao foco (um commit muda a base). */
export function margemGit(ed: Ed, caminho: () => string | null): () => void {
  const decs = ed.createDecorationsCollection();
  let base: string | null = null;
  let de = "";
  let espera = 0;
  const marca = (ln: number, cls: string): monaco.editor.IModelDeltaDecoration => ({ range: new monaco.Range(ln, 1, ln, 1), options: { isWholeLine: true, linesDecorationsClassName: cls } });
  const pintar = () => {
    const model = ed.getModel();
    if (!model || base === null) return decs.clear();
    const m = diffLinhas(base, model.getValue());
    decs.set([
      ...m.novas.map((l) => marca(l, "git-margem nova")),
      ...m.alteradas.map((l) => marca(l, "git-margem alterada")),
      ...m.removidasApos.map((l) => marca(Math.max(1, l), l === 0 ? "git-margem removida-topo" : "git-margem removida")),
    ]);
  };
  const carregar = () => {
    const p = caminho();
    de = p ?? "";
    if (!p) { base = null; return pintar(); }
    api.fsGitBase(p).then((b) => { if (de === p) { base = b; pintar(); } }).catch(() => { if (de === p) { base = null; pintar(); } });
  };
  const agendar = () => { window.clearTimeout(espera); espera = window.setTimeout(pintar, 250); };
  const subs = [ed.onDidChangeModelContent(agendar), ed.onDidChangeModel(() => carregar())];
  window.addEventListener("focus", carregar);
  window.addEventListener("regente:arquivo-salvo", carregar);
  carregar();
  return () => {
    subs.forEach((s) => s.dispose());
    window.clearTimeout(espera);
    window.removeEventListener("focus", carregar);
    window.removeEventListener("regente:arquivo-salvo", carregar);
    decs.clear();
  };
}
