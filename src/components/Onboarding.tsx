import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, errMsg } from "../lib/ipc";
import { defaultAgentLabel, firstMissionInput } from "../lib/quickstart";
import { readLastCli } from "../lib/lastAgent";
import { useStore, wsOf } from "../state/store";
import Modal from "./Modal";
import { spawnInActive } from "./settings/Settings";
import InstallButton from "./InstallButton";
import { installSpec } from "../lib/install";

const MAIN = ["claude", "codex", "gemini"];
const TRUST_HINT = "Se pedir para confiar na pasta, responda 1 e Enter.";

export default function Onboarding() {
  const [step, setStep] = useState(0);
  const settings = useStore((s) => s.settings);
  const detected = useStore((s) => s.detected);
  const providers = useStore((s) => s.providers);
  const active = useStore((s) => s.active);
  if (!settings) return null;
  const { saveSettings, openWorkspace, patch, toast } = useStore.getState();
  const finish = () => {
    patch({ onboardingOpen: false });
    return saveSettings({ ...settings, onboardingDone: true }).catch((e) => toast(errMsg(e), "erro"));
  };
  async function pick() {
    const dir = await open({ directory: true, title: "Escolher pasta de trabalho" });
    if (typeof dir === "string" && (await openWorkspace(dir))) setStep(2);
  }
  async function install(label: string, run: string) {
    if (await spawnInActive(installSpec(label, run))) toast(`Terminal de instalação do ${label} aberto. Quando terminar, clique em Detectar de novo.`, "ok");
  }
  // a mesma IA padrão dos cards de início rápido ({ia}): a última escolhida se instalada, senão a primeira instalada
  const agent = defaultAgentLabel(detected, readLastCli(), providers?.clis ?? []);
  const cliId = agent.cli;
  const cli = providers?.clis.find((c) => c.id === cliId);
  async function openFirst() {
    if (!cli || !(await spawnInActive({ kind: "agent", cli: cli.id, permission: "pend" }))) return;
    finish();
    toast(TRUST_HINT, "info");
  }
  async function twoPanes() {
    const ws = wsOf(useStore.getState().active);
    if (!ws) return;
    try {
      const m = await api.missionCreate(firstMissionInput(ws.id, ws.path, cliId));
      await useStore.getState().loadMissions(ws.id);
      useStore.getState().selectMission(ws.id, m.id);
      finish();
      toast(TRUST_HINT, "info");
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  return (
    <Modal label="Boas-vindas ao Regente" onClose={finish} backdropCloses={false}>
      {step === 0 && (
        <>
          <h2>Bem-vindo ao Regente</h2>
          <p>Um cockpit para rodar várias IAs de código lado a lado: Claude Code, Codex, Gemini e outras, cada uma no seu terminal, com o custo de cada uma ao vivo.</p>
          <p>O Regente usa os logins que já estão neste computador e não cobra pelos modelos.</p>
          <div className="actions"><button className="btn" onClick={finish}>Pular</button><button className="btn primary" onClick={() => setStep(1)}>Começar</button></div>
        </>
      )}
      {step === 1 && (
        <>
          <h2>Escolher pasta de trabalho</h2>
          <p>Abra a pasta do projeto em que as IAs vão trabalhar. Ela vira uma aba.</p>
          <div className="actions"><button className="btn" onClick={() => setStep(2)}>Depois</button><button className="btn primary" onClick={() => pick().catch((e) => toast(errMsg(e), "erro"))}>Escolher pasta</button></div>
        </>
      )}
      {step === 2 && (
        <>
          <h2>Escolha seus assistentes</h2>
          <p>Tudo é opcional. Dá para instalar agora ou depois em Configurações → Suas IAs.</p>
          {providers?.clis.filter((c) => MAIN.includes(c.id)).map((c) => {
            const d = detected.find((x) => x.id === c.id && x.kind === "cli");
            return (
              <div key={c.id} className="card row" style={{ justifyContent: "space-between" }}>
                <span><strong>{c.label}</strong> <span className="chip">{d?.path ? "instalado" : "não encontrado"}</span></span>
                {!d?.path && c.install && (
                  <InstallButton label={c.label} command={c.install} disabled={!active} disabledReason="Escolha uma pasta primeiro" onConfirm={() => install(c.label, c.install!)} />
                )}
              </div>
            );
          })}
          <div className="actions">
            <button className="btn" onClick={() => api.providersDetect().then((d) => patch({ detected: d })).catch((e) => toast(errMsg(e), "erro"))}>Detectar de novo</button>
            <button className="btn primary" onClick={() => setStep(3)}>Continuar</button>
          </div>
        </>
      )}
      {step === 3 && (
        <>
          <h2>Abrir seu primeiro painel</h2>
          {!active && <p className="meta">Escolha uma pasta primeiro: os painéis abrem nela.</p>}
          {cli && detected.some((d) => d.id === cli.id && d.path) ? (
            <div className="actions">
              <button className="btn primary" disabled={!active} onClick={() => openFirst().catch((e) => toast(errMsg(e), "erro"))}>Abrir {cli.label} agora</button>
            </div>
          ) : (
            cli?.install && <p><InstallButton label={cli.label} command={cli.install} disabled={!active} disabledReason="Escolha uma pasta primeiro" onConfirm={() => install(cli.label, cli.install!)} /></p>
          )}
          <div className="actions">
            <button className="btn" disabled={!active} onClick={() => twoPanes().catch((e) => toast(errMsg(e), "erro"))}>2 painéis lado a lado</button>
            <button className="btn" onClick={finish}>Fechar</button>
          </div>
        </>
      )}
    </Modal>
  );
}
