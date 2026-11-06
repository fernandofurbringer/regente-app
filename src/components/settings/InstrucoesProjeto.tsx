import { useEffect, useState } from "react";
import { api, errMsg, type Instrucao, type SugestaoInstrucao } from "../../lib/ipc";
import { useStore } from "../../state/store";
import { DiffView } from "../MissionReview";
import { editorTheme } from "../../lib/theme";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");

/** Instruções do projeto (CLAUDE.md, AGENTS.md, copilot): leitura, sugestão da IA auxiliar em diff, e gravação só com "Aplicar". */
export default function InstrucoesProjeto({ wsId }: { wsId: string | null }) {
  const theme = useStore((s) => s.settings?.theme);
  const [lista, setLista] = useState<Instrucao[] | null>(null);
  const [sugestao, setSugestao] = useState<{ arquivo: string; hash: string; sugerido: SugestaoInstrucao } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const recarregar = (id: string) => api.instrucoesListar(id).then(setLista, toast);

  useEffect(() => {
    setSugestao(null);
    if (!wsId) { setLista([]); return; }
    setLista(null);
    recarregar(wsId);
  }, [wsId]);

  if (!wsId) return <div className="meta">Escolha um workspace.</div>;

  async function sugerir(i: Instrucao) {
    setBusy(i.arquivo);
    try {
      const sugerido = await api.instrucoesSugerir(wsId!, i.arquivo);
      setSugestao({ arquivo: i.arquivo, hash: i.hash, sugerido });
    } catch (e) {
      toast(e);
    } finally {
      setBusy(null);
    }
  }

  async function aplicar() {
    if (!sugestao) return;
    try {
      await api.instrucoesAplicar(wsId!, sugestao.arquivo, sugestao.sugerido.proposto, sugestao.hash);
      setSugestao(null);
      useStore.getState().toast(`${sugestao.arquivo} atualizado.`, "info");
      recarregar(wsId!);
    } catch (e) {
      toast(e);
    }
  }

  if (lista === null) return <div className="meta" aria-live="polite">Carregando instruções…</div>;
  if (lista.length === 0) {
    return <div className="meta">Nenhum CLAUDE.md, AGENTS.md ou .github/copilot-instructions.md na raiz deste workspace.</div>;
  }

  return (
    <>
      <p className="meta">Os arquivos são lidos do workspace. A sugestão usa as memórias aceitas (decisões e fatos) e só grava quando você clica em Aplicar.</p>
      {lista.map((i) => (
        <section key={i.arquivo} className="card" style={{ marginBottom: 8 }}>
          <h4>{i.arquivo}</h4>
          <pre style={{ whiteSpace: "pre-wrap", maxHeight: 200, overflow: "auto" }} aria-label={`Conteúdo de ${i.arquivo}`}>{i.texto}</pre>
          <button className="btn" disabled={busy !== null} onClick={() => sugerir(i)}>
            {busy === i.arquivo ? "Sugerindo…" : "Sugerir atualização a partir da memória"}
          </button>
          {sugestao?.arquivo === i.arquivo && (
            <div style={{ marginTop: 8 }}>
              <div style={{ height: 360 }}>
                <DiffView original={sugestao.sugerido.original} modified={sugestao.sugerido.proposto} language="markdown" theme={editorTheme(theme)} />
              </div>
              <div className="row" style={{ marginTop: 6, gap: 6 }}>
                <button className="btn primary" onClick={aplicar}>Aplicar</button>
                <button className="btn" onClick={() => setSugestao(null)}>Descartar</button>
              </div>
            </div>
          )}
        </section>
      ))}
    </>
  );
}
