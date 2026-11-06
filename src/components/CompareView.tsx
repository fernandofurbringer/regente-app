import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errMsg, type ChangedFile, type CompareResumo, type CompareVariante, type FileVersions, type Permission } from "../lib/ipc";
import { langOf } from "../lib/review";
import { useStore } from "../state/store";
import Modal from "./Modal";
import { DiffView } from "./MissionReview";
import { PERMISSIONS } from "./NewPaneDialog";
import "./CompareView.css";
import { editorTheme } from "../lib/theme";

const MIN = 2;
const MAX = 4;
const RESUMO_MS = 3000;
const gb = (b: number) => `${(b / 1024 ** 3).toFixed(1)} GB`;
const mb = (b: number) => `${Math.round(b / 1024 ** 2)} MB`;

interface Linha { label: string; cli: string; model: string }
interface Espaco { livreBytes: number | null; estimadoBytes: number }

/** "Comparar abordagens" (A1c, RM-42): o mesmo pedido em 2 a 4 variantes, depois o resumo e "Ficar com esta". */
export default function CompareView() {
  const cmp = useStore((s) => s.compare);
  const close = () => useStore.getState().patch({ compare: null });
  if (!cmp) return null;
  return (
    <Modal label="Comparar abordagens" onClose={close} backdropCloses={false} style={{ width: "min(1100px, 96vw)" }}>
      {cmp.groupId ? <Resultado groupId={cmp.groupId} close={close} /> : <Formulario workspaceId={cmp.workspaceId} />}
    </Modal>
  );
}

function Formulario({ workspaceId }: { workspaceId: string }) {
  const clis = useStore((s) => s.providers?.clis ?? []);
  const toast = useStore((s) => s.toast);
  const [nome, setNome] = useState("");
  const [pedido, setPedido] = useState("");
  const [linhas, setLinhas] = useState<Linha[]>(() => [
    { label: "A", cli: clis[0]?.id ?? "claude", model: "" },
    { label: "B", cli: clis[0]?.id ?? "claude", model: "" },
  ]);
  const [permissao, setPermissao] = useState<Permission>("pend");
  const [espaco, setEspaco] = useState<Espaco | null>(null);
  const [busy, setBusy] = useState(false);
  const set = (i: number, patch: Partial<Linha>) => setLinhas((ls) => ls.map((l, j) => (j === i ? { ...l, ...patch } : l)));

  useEffect(() => {
    api.compareEspaco(workspaceId).then(setEspaco).catch(() => setEspaco(null));
  }, [workspaceId]);
  // espaço livre abaixo de 2× o necessário para as cópias: aviso (a criação só recusa se não couber)
  const n = linhas.length;
  const aviso = espaco && espaco.livreBytes !== null && espaco.livreBytes < 2 * n * espaco.estimadoBytes
    ? `Espaço livre baixo: ${gb(espaco.livreBytes)} livres para ${n} cópias de cerca de ${mb(espaco.estimadoBytes)} cada.`
    : null;

  async function comecar() {
    setBusy(true);
    try {
      const variantes = linhas.map((l) => ({ label: l.label.trim(), cli: l.cli, model: l.model.trim() || null }));
      const criadas = await api.compareStart(workspaceId, nome, pedido, variantes, permissao);
      useStore.getState().patch({ compare: { workspaceId, groupId: criadas[0].groupId } });
    } catch (e) {
      toast(errMsg(e), "erro");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="compare-form">
      <label className="compare-label">Nome da comparação
        <input className="compare-input" value={nome} onChange={(e) => setNome(e.target.value)} placeholder="Ex.: tela de login" />
      </label>
      <label className="compare-label">Pedido (igual para todas as variantes)
        <textarea className="compare-input" rows={4} value={pedido} onChange={(e) => setPedido(e.target.value)} />
      </label>
      {linhas.map((l, i) => (
        <div className="compare-row" key={i}>
          <input className="compare-input" aria-label={`Nome da variante ${i + 1}`} value={l.label} onChange={(e) => set(i, { label: e.target.value })} style={{ maxWidth: 120 }} />
          <select className="compare-input" aria-label={`CLI da variante ${i + 1}`} value={l.cli} onChange={(e) => set(i, { cli: e.target.value })}>
            {clis.map((c) => <option key={c.id} value={c.id}>{c.label}</option>)}
          </select>
          <input className="compare-input" aria-label={`Modelo da variante ${i + 1}`} placeholder="modelo (opcional)" value={l.model} onChange={(e) => set(i, { model: e.target.value })} />
          {linhas.length > MIN && <button className="btn" onClick={() => setLinhas((ls) => ls.filter((_, j) => j !== i))}>Remover</button>}
        </div>
      ))}
      <label className="compare-label">Permissão das variantes
        <select className="compare-input" aria-label="Permissão das variantes" value={permissao} onChange={(e) => setPermissao(e.target.value as Permission)}>
          {PERMISSIONS.map((p) => <option key={p.id} value={p.id}>{p.label} — {p.hint}</option>)}
        </select>
      </label>
      {aviso && <p className="meta" role="alert">{aviso}</p>}
      <div className="row" style={{ justifyContent: "space-between", gap: 8 }}>
        <button className="btn" disabled={linhas.length >= MAX} onClick={() => setLinhas((ls) => [...ls, { label: String.fromCharCode(65 + ls.length), cli: clis[0]?.id ?? "claude", model: "" }])}>+ variante</button>
        <button className="btn primary" disabled={busy || !pedido.trim()} onClick={comecar}>Começar</button>
      </div>
      <p className="meta">Cada variante cria a própria cópia de trabalho (worktree) do projeto. Com {MAX} variantes o disco guarda até {MAX} cópias: confira o espaço livre antes.</p>
    </div>
  );
}

function Resultado({ groupId, close }: { groupId: string; close: () => void }) {
  const [resumo, setResumo] = useState<CompareResumo | null>(null);
  const toast = useStore((s) => s.toast);
  const theme = useStore((s) => s.settings?.theme ?? "dark");

  const pronta = resumo?.pronta ?? false;

  useEffect(() => {
    let vivo = true;
    const ler = () => api.compareGet(groupId).then((r) => vivo && setResumo(r)).catch((e) => vivo && toast(errMsg(e), "erro"));
    ler();
    if (pronta) return () => { vivo = false; };
    const t = setInterval(ler, RESUMO_MS);
    return () => { vivo = false; clearInterval(t); };
  }, [groupId, pronta, toast]);

  if (!resumo) return <div className="meta">Lendo a comparação…</div>;

  async function ficar(v: CompareVariante) {
    const outras = resumo!.variantes.filter((o) => o.missionId !== v.missionId);
    const lista = outras.map((o) => `${o.label} (+${o.add} −${o.del})`).join(", ");
    const ok = await ask(`Integrar "${v.label}" (+${v.add} −${v.del}) no projeto e arquivar as outras: ${lista}. Os worktrees das outras variantes serão removidos.`, { title: "Ficar com esta?", kind: "warning", okLabel: "Continuar", cancelLabel: "Cancelar" });
    if (!ok) return;
    const comMudancas = outras.filter((o) => o.uncommitted > 0);
    if (comMudancas.length > 0) {
      // o arquivamento grava essas mudanças num commit wip na branch da variante: não somem, mas não entram no projeto
      const aviso = comMudancas.map((o) => `⚠ ${o.uncommitted} arquivos não commitados em "${o.label}" serão arquivados num commit wip na branch da variante (não entram no projeto).`).join("\n");
      const ok2 = await ask(`${aviso}\n\nConfirmar mesmo assim?`, { title: "Mudanças não commitadas", kind: "warning", okLabel: "Ficar com esta", cancelLabel: "Cancelar" });
      if (!ok2) return;
    }
    try {
      const r = await api.compareKeep(groupId, v.missionId);
      if (r.falhas.length === 0) {
        toast(`Variante "${v.label}" integrada; as outras foram arquivadas.`, "ok");
        close();
        return;
      }
      // integrada, mas alguma perdedora não arquivou: o resumo mostra o que sobrou, e um novo clique tenta só essas
      toast(`Integrada; não consegui arquivar: ${r.falhas.join(", ")} (tente de novo)`, "erro");
      setResumo(await api.compareGet(groupId));
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }

  return (
    <div className="compare-result">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
        <strong>Comparação</strong>
        <span className="meta">{resumo.pronta ? "Todas as variantes terminaram" : "Aguardando as variantes terminarem…"} · total {resumo.total.files} arq. +{resumo.total.add} −{resumo.total.del} · US$ {resumo.total.costUsd.toFixed(2)}</span>
      </div>
      <table className="compare-table">
        <thead>
          <tr><th>Variante</th><th>Estado</th><th>Arquivos</th><th>Custo</th><th>Tempo</th><th>Verificação</th><th /></tr>
        </thead>
        <tbody>
          {resumo.variantes.map((v) => (
            <tr key={v.missionId}>
              <td>{v.label}</td>
              <td>{v.working ? "trabalhando" : v.status}</td>
              <td>{v.files} arq. <span className="meta">+{v.add} −{v.del}</span></td>
              <td>US$ {v.costUsd.toFixed(2)}</td>
              <td>{Math.round(v.durationMs / 1000)} s</td>
              <td>{v.verify ? <span className={`badge ${v.verify.resultado.ok ? "ok" : "erro"}`}>{v.verify.resultado.ok ? "✔" : "✘"} {Math.round(v.verify.resultado.durationMs / 1000)} s</span> : <span className="meta">—</span>}</td>
              <td>{resumo.pronta && <button className="btn" onClick={() => ficar(v)}>Ficar com esta</button>}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <Lado resumo={resumo} theme={theme} />
    </div>
  );
}

/** Diff lado a lado de um arquivo entre duas variantes (conteúdo atual de cada uma). */
function Lado({ resumo, theme }: { resumo: CompareResumo; theme: string }) {
  const [arquivos, setArquivos] = useState<string[] | null>(null);
  const [a, setA] = useState(resumo.variantes[0]?.missionId ?? "");
  const [b, setB] = useState(resumo.variantes[1]?.missionId ?? "");
  const [file, setFile] = useState<string | null>(null);
  const [versoes, setVersoes] = useState<{ a: FileVersions; b: FileVersions } | null>(null);

  useEffect(() => {
    Promise.all(resumo.variantes.map((v) => api.missionDiffFiles(v.missionId).catch(() => [] as ChangedFile[])))
      .then((listas) => setArquivos([...new Set(listas.flat().map((f) => f.path))].sort()))
      .catch(() => setArquivos([]));
  }, [resumo.variantes.length]);

  useEffect(() => {
    if (!file || !a || !b) return;
    setVersoes(null);
    Promise.all([api.missionFileVersions(a, file), api.missionFileVersions(b, file)])
      .then(([va, vb]) => setVersoes({ a: va, b: vb }))
      .catch(() => setVersoes(null));
  }, [file, a, b]);

  if (arquivos === null) return <div className="meta">Lendo os arquivos…</div>;
  if (arquivos.length === 0) return <div className="empty">Nenhuma variante mudou arquivos ainda.</div>;
  const nome = (id: string) => resumo.variantes.find((v) => v.missionId === id)?.label ?? id;

  return (
    <div className="compare-side">
      <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
        <select className="compare-input" aria-label="Arquivo" value={file ?? ""} onChange={(e) => setFile(e.target.value || null)}>
          <option value="">Escolha um arquivo</option>
          {arquivos.map((p) => <option key={p} value={p}>{p}</option>)}
        </select>
        <select className="compare-input" aria-label="Variante A" value={a} onChange={(e) => setA(e.target.value)}>
          {resumo.variantes.map((v) => <option key={v.missionId} value={v.missionId}>{v.label}</option>)}
        </select>
        <select className="compare-input" aria-label="Variante B" value={b} onChange={(e) => setB(e.target.value)}>
          {resumo.variantes.map((v) => <option key={v.missionId} value={v.missionId}>{v.label}</option>)}
        </select>
      </div>
      {file && !versoes && <div className="meta">Carregando o diff…</div>}
      {file && versoes && (versoes.a.binary || versoes.b.binary) && <div className="empty">Arquivo binário: sem diff de texto.</div>}
      {file && versoes && !(versoes.a.binary || versoes.b.binary) && (
        <div className="compare-diff">
          <div className="meta">{nome(a)} ← → {nome(b)}</div>
          <DiffView original={versoes.a.current} modified={versoes.b.current} language={langOf(file)} theme={editorTheme(theme)} />
        </div>
      )}
    </div>
  );
}
