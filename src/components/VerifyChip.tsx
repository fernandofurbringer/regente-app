import { useEffect, useState } from "react";
import { api, errMsg, on, type VerifyCfg, type VerifyResultado, type VerifyVisao } from "../lib/ipc";
import { useStore } from "../state/store";
import Modal from "./Modal";
import "./VerifyChip.css";

type Ultimo = { resultado: VerifyResultado; modo: string; em: number } | null;
interface EventoEstado { missionId: string; rodando: boolean; ultimo?: Ultimo }

/** Duração curta para o chip: `42 s`, `1m10`. */
export function duracao(ms: number): string {
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}`;
}

/** Chip da missão na barra: `✔ testes 42 s`, `✘ falhou · 1m10`, `rodando…`. Clique foca o painel de verificação. */
export function VerifyChip({ missionId }: { workspaceId: string; missionId: string }) {
  const [visao, setVisao] = useState<VerifyVisao | null>(null);
  const [rodando, setRodando] = useState(false);
  const [ultimo, setUltimo] = useState<Ultimo>(null);
  useEffect(() => {
    let vivo = true;
    setVisao(null);
    api.missionVerifyGet(missionId).then((v) => {
      if (!vivo) return;
      setVisao(v);
      setRodando(v.rodando);
      setUltimo(v.ultimo);
    }).catch(() => { /* chip é informativo: sem ele a barra segue igual */ });
    const un = on<EventoEstado>("verificacao-estado", (e) => {
      if (e.missionId !== missionId) return;
      setRodando(e.rodando);
      if (e.ultimo !== undefined) setUltimo(e.ultimo);
    });
    return () => { vivo = false; un.then((f) => f()); };
  }, [missionId]);

  // sem comando e sem rodada anterior, a barra não ganha chip
  if (!visao || (!visao.cfg.comando && !visao.comandoProjeto && !ultimo && !rodando)) return null;
  const texto = rodando ? "rodando…" : ultimo ? (ultimo.resultado.ok ? `✔ testes ${duracao(ultimo.resultado.durationMs)}` : `✘ falhou · ${duracao(ultimo.resultado.durationMs)}`) : "verificação";
  const classe = rodando ? "" : ultimo ? (ultimo.resultado.ok ? "verify-ok" : "verify-erro") : "";
  const foca = () => { if (visao.painel) useStore.getState().patch({ activePane: visao.painel }); };
  return (
    <button className={`badge verify-chip ${classe}`} title="Verificação da missão: abre a saída" onClick={foca} disabled={!visao.painel}>
      {texto}
    </button>
  );
}

/** Diálogo "Verificação…" do menu da missão: comando (vem preenchido do projeto), toggles e rodar. */
export function VerifyDialog() {
  const missionId = useStore((s) => s.verifyMission);
  if (!missionId) return null;
  return (
    <Modal label="Verificação da missão" onClose={() => useStore.getState().patch({ verifyMission: null })}>
      <VerifyForm missionId={missionId} />
    </Modal>
  );
}

function VerifyForm({ missionId }: { missionId: string }) {
  const [visao, setVisao] = useState<VerifyVisao | null>(null);
  const [comando, setComando] = useState("");
  const [rodarAoTerminar, setRodarAoTerminar] = useState(false);
  const [autoLoop, setAutoLoop] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [rodando, setRodando] = useState(false);

  useEffect(() => {
    let vivo = true;
    api.missionVerifyGet(missionId).then((v) => {
      if (!vivo) return;
      setVisao(v);
      setComando(v.cfg.comando ?? v.comandoProjeto ?? "");
      setRodarAoTerminar(v.cfg.rodarAoTerminar);
      setAutoLoop(v.cfg.autoLoop);
      setRodando(v.rodando);
    }).catch((e) => setMsg(errMsg(e)));
    const un = on<EventoEstado>("verificacao-estado", (e) => { if (e.missionId === missionId) setRodando(e.rodando); });
    return () => { vivo = false; un.then((f) => f()); };
  }, [missionId]);

  if (!visao) return <p className="verify-msg">{msg ?? "Carregando…"}</p>;
  const doProjeto = !visao.cfg.comando && !!visao.comandoProjeto;
  const precisaConfianca = doProjeto && !visao.confiado;

  // o comando do projeto só vira da missão se for diferente do que o projeto define (assim a confiança continua valendo)
  const salvar = async (): Promise<VerifyVisao> => {
    const texto = comando.trim();
    const cfg: VerifyCfg = {
      ...visao.cfg,
      comando: texto && texto !== visao.comandoProjeto ? texto : null,
      rodarAoTerminar,
      autoLoop,
    };
    const nova = await api.missionVerifySet(missionId, cfg);
    setVisao(nova);
    return nova;
  };

  const rodar = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await salvar();
      if (precisaConfianca) {
        // o usuário confere o comando aqui, na tela, antes de o projeto ser confiado
        const info = await api.projetoGet(visao.workspaceId);
        await api.projetoTrust(visao.workspaceId, true, info.hash);
      }
      const r = await api.missionVerifyRun(missionId);
      setMsg(r.ok ? `Passou em ${duracao(r.durationMs)}.` : `Falhou em ${duracao(r.durationMs)}${r.exitCode != null ? ` (código ${r.exitCode})` : ""}. A saída está no painel Verificação.`);
      setVisao(await api.missionVerifyGet(missionId));
    } catch (e) {
      setMsg(errMsg(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="verify-form">
      <label className="verify-label">Comando de verificação
        <input className="verify-input" value={comando} onChange={(e) => setComando(e.target.value)} placeholder="npm test" spellCheck={false} />
      </label>
      {doProjeto && <p className="verify-hint">Vem de .regente/projeto.json{visao.confiado ? "" : " (ainda não confiado: o Regente pede a sua confirmação antes de rodar)"}.</p>}
      <label className="verify-check"><input type="checkbox" checked={rodarAoTerminar} onChange={(e) => setRodarAoTerminar(e.target.checked)} /> Rodar ao terminar um agente</label>
      <label className="verify-check"><input type="checkbox" checked={autoLoop} onChange={(e) => setAutoLoop(e.target.checked)} /> Corrigir automaticamente (até {visao.cfg.maxIter} vezes)</label>
      <div className="verify-actions">
        <button className="btn" onClick={() => { setBusy(true); salvar().then(() => setMsg("Salvo.")).catch((e) => setMsg(errMsg(e))).finally(() => setBusy(false)); }} disabled={busy}>Salvar</button>
        {rodando
          ? <button className="btn" onClick={() => { void api.missionVerifyCancel(missionId); }}>Cancelar rodada</button>
          : <button className="btn primary" onClick={rodar} disabled={busy || (!comando.trim() && !visao.comandoProjeto)}>
              {precisaConfianca ? "Confiar e rodar" : "Rodar verificação"}
            </button>}
      </div>
      {visao.ultimo && <p className="verify-msg">Última rodada: {visao.ultimo.resultado.ok ? "passou" : "falhou"} em {duracao(visao.ultimo.resultado.durationMs)} ({visao.ultimo.modo}).</p>}
      {msg && <p className="verify-msg">{msg}</p>}
    </div>
  );
}

/** Avisos de verificação, fora da barra (valem para qualquer missão): falha com ação, confiança do projeto, limite. */
const confiancaPedida = new Set<string>();
export function useVerifyToasts() {
  useEffect(() => {
    const uns = [
      on<{ missionId: string; digest: string; automatica: boolean; limite: boolean; tentativa: number }>("verificacao-falhou", (e) => {
        const { toast } = useStore.getState();
        if (e.limite) toast("Verificação falhou de novo; o ciclo automático parou.", "erro");
        else if (e.automatica) toast(`Verificação falhou (tentativa ${e.tentativa}): a correção foi enviada ao agente.`, "info");
        else toast("Verificação falhou", "erro", { label: "Enviar falha ao agente", run: () => { api.missionVerifyEnviar(e.missionId).catch((err) => useStore.getState().toast(errMsg(err), "erro")); } }, 12000);
      }),
      on<{ workspaceId: string }>("verificacao-confiar", (e) => {
        if (confiancaPedida.has(e.workspaceId)) return;
        confiancaPedida.add(e.workspaceId);
        useStore.getState().toast("Este projeto define comandos de teste. Confiar e rodar a verificação?", "info", {
          label: "Confiar e rodar",
          run: () => { api.projetoGet(e.workspaceId).then((info) => api.projetoTrust(e.workspaceId, true, info.hash)).then(() => useStore.getState().toast("Projeto confiado", "ok")).catch((err) => useStore.getState().toast(errMsg(err), "erro")); },
        }, 15000);
      }),
    ];
    return () => { uns.forEach((p) => { p.then((f) => f()); }); };
  }, []);
}
