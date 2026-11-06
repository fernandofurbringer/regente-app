import { useEffect, useState } from "react";
import { api, on, type PerfilEfetivo, type PerfisInfo } from "../lib/ipc";
import "./ProfileChip.css";

/** Presets e ajustes mudam pouco: uma leitura por sessão da janela, compartilhada pelos chips. */
let info: Promise<PerfisInfo> | null = null;
const presets = () => (info ??= api.perfisGet().catch((e) => { info = null; throw e; }));
const presetsReset = () => { info = null; };

/** Chaves tocadas por um merge-patch, com pontos: `{saidas: {bashMax: 5}}` -> `saidas.bashMax`. */
function chaves(obj: unknown, prefixo = ""): string[] {
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) return [];
  return Object.entries(obj as Record<string, unknown>).flatMap(([k, v]) => (v && typeof v === "object" && !Array.isArray(v) ? chaves(v, `${prefixo}${k}.`) : [`${prefixo}${k}`]));
}

interface Props {
  workspaceId: string;
  missionId: string | null;
  /** perfil que o painel abriu (id); sem ele, mostra o efetivo da missão/workspace */
  profileId?: string | null;
  /** painel: o recibo do que foi aplicado na abertura (tooltip) */
  paneId?: string | null;
}

/** Chip de perfil: `Equilibrado`, com `*` quando o workspace tem ajuste. O tooltip diz a origem e o que foi ajustado. */
export default function ProfileChip({ workspaceId, missionId, profileId, paneId }: Props) {
  const [ef, setEf] = useState<{ eff: PerfilEfetivo; nome: string; ajustes: string[] } | null>(null);
  const [recibo, setRecibo] = useState<string[]>([]);
  // muda quando um perfil ou ajuste muda (Configurações, desfazer, ajuste automático)
  const [rev, setRev] = useState(0);
  useEffect(() => {
    const uns = ["perfis-changed", "ajuste-novo", "ajuste-desfeito"].map((ev) => on(ev, () => { presetsReset(); setRev((r) => r + 1); }));
    return () => { uns.forEach((un) => { un.then((f) => f()); }); };
  }, []);
  useEffect(() => {
    if (!paneId) return;
    api.panePerfil(paneId).then(setRecibo).catch(() => setRecibo([]));
  }, [paneId, rev]);
  useEffect(() => {
    let live = true;
    Promise.all([api.perfilEfetivo(workspaceId, missionId), presets()])
      .then(([eff, pi]) => {
        if (!live) return;
        const nome = pi.presets.find((p) => p.id === (profileId ?? eff.perfil.id))?.nome ?? eff.perfil.nome;
        setEf({ eff, nome, ajustes: chaves(pi.patches[workspaceId]) });
      })
      .catch(() => { /* chip é informativo: sem ele a barra segue igual */ });
    return () => { live = false; };
  }, [workspaceId, missionId, profileId, rev]);
  // painel sem perfil gravado (anterior ao perfil de trabalho): sem chip de perfil, só o traço
  if (paneId && !profileId) return <span className="badge profile-chip" title="Painel sem perfil registrado">—</span>;
  if (!ef) return null;
  const origem = ef.eff.origem === "missao" ? "da missão" : "padrão";
  const ajuste = ef.eff.ajustado ? `ajustado: ${ef.ajustes.join(", ") || "sim"}` : "sem ajustes";
  const aplicado = recibo.length ? ` · Aplicado: ${recibo.join(", ")}` : "";
  return (
    <span className="badge profile-chip" title={`Perfil: ${ef.nome} (${origem}) · ${ajuste}${aplicado}`}>
      {ef.nome}{ef.eff.ajustado ? " *" : ""}
    </span>
  );
}
