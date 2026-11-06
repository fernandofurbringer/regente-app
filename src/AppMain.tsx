import { lazy, Suspense, useEffect } from "react";
import NewMissionWizard from "./components/NewMissionWizard";
import Onboarding from "./components/Onboarding";
import NewPaneDialog from "./components/NewPaneDialog";
import { SessionPickerDialog } from "./components/SessionPicker";
import GateModal, { GateStrip } from "./components/GateModal";
import Home from "./components/Home";
import Toasts from "./components/Toasts";
import JarvisConfirmModal from "./components/JarvisConfirmModal";
import MobilePairModal from "./components/MobilePairModal";
import WhatsNewModal from "./components/WhatsNewModal";
// ▼ trilha C
import { useAwayFocus } from "./lib/awayFocus";
// ▲ trilha C
import TopBar from "./components/TopBar";
import WorkspaceTabs from "./components/WorkspaceTabs";
import GlobalVoice from "./components/GlobalVoice";
import CommandPalette, { useAppShortcuts } from "./components/CommandPalette";
import { startVisibilityReport } from "./lib/visible";
import ConversationExport from "./components/ConversationExport";
import SendToAgentDialog from "./components/SendToAgent";
import GitDialog from "./components/GitDialog";
import { VerifyDialog, useVerifyToasts } from "./components/VerifyChip";
import PaneSearch from "./components/PaneSearch";
import { ModelSwitchDialog } from "./components/ModelSwitch";
import WorkspaceView from "./components/WorkspaceView";
import { RestoreBanner } from "./components/RestorePrompt";
import { errMsg } from "./lib/ipc";
import { useStore } from "./state/store";
import { useAppEvents, useFileDrop } from "./appEvents";
// ▼ trilha B
import { useCheckpointTrigger } from "./lib/checkpointTrigger";
// ▲ trilha B

// a revisão tem Monaco (diff) e as configurações têm 21 seções: só entram quando abertas
const MissionReview = lazy(() => import("./components/MissionReview"));
const Settings = lazy(() => import("./components/settings/Settings"));
// ▼ trilha B
const Timeline = lazy(() => import("./components/Timeline"));
// ▼ trilha C
const CompareView = lazy(() => import("./components/CompareView"));
const AwayDigest = lazy(() => import("./components/AwayDigest"));
// ▲ trilha C
// ▲ trilha B

function MainWindow() {
  const tabs = useStore((s) => s.tabs);
  const active = useStore((s) => s.active);
  const dialog = useStore((s) => s.dialog);
  const settings = useStore((s) => s.settings);
  const onboardingOpen = useStore((s) => s.onboardingOpen);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const sessionPicker = useStore((s) => s.sessionPicker);
  const reviewOpen = useStore((s) => !!s.reviewMission);
  const timelineOpen = useStore((s) => !!s.timeline);
  // ▼ trilha C
  const compareOpen = useStore((s) => !!s.compare);
  const digestOpen = useStore((s) => s.digestSince !== null);
  useAwayFocus();
  // ▲ trilha C
  return (
    <div className="app">
      <TopBar />
      <WorkspaceTabs />
      {/* fora do app-body: o workspace ocupa o app-body inteiro (inset: 0) e cobriria estas faixas */}
      <RestoreBanner />
      <GateStrip />
      <div className="app-body">
        {/* o Início rola sozinho: o app-body não rola (os workspaces ocupam ele inteiro) e a lista cortava embaixo */}
        {active === null && <div className="home-scroll"><Home /></div>}
        {tabs.map((id) => <WorkspaceView key={id} wsId={id} hidden={id !== active} />)}
      </div>
      {dialog === "painel" && active && <NewPaneDialog wsId={active} />}
      {dialog === "missao" && active && <NewMissionWizard wsId={active} />}
      {sessionPicker && <SessionPickerDialog wsId={sessionPicker} />}
      <SendToAgentDialog />
      <GateModal />
      {settings && (!settings.onboardingDone || onboardingOpen) && <Onboarding />}
      {settingsOpen && <Suspense fallback={null}><Settings /></Suspense>}
      <JarvisConfirmModal />
      <MobilePairModal />
      {!window.location.hash.startsWith("#/") && <WhatsNewModal />}
      <GlobalVoice />
      <CommandPalette />
      <ConversationExport />
      {reviewOpen && <Suspense fallback={null}><MissionReview /></Suspense>}
      {/* ▼ trilha B */}
      {timelineOpen && <Suspense fallback={null}><Timeline /></Suspense>}
      {/* ▲ trilha B */}
      {/* ▼ trilha C */}
      {compareOpen && <Suspense fallback={null}><CompareView /></Suspense>}
      {digestOpen && <Suspense fallback={null}><AwayDigest /></Suspense>}
      {/* ▲ trilha C */}
      <GitDialog />
      <VerifyDialog />
      <ModelSwitchDialog />
      <PaneSearch />
      <Toasts />
    </div>
  );
}

function WorkspaceWindow({ wsId }: { wsId: string }) {
  const dialog = useStore((s) => s.dialog);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const reviewOpen = useStore((s) => !!s.reviewMission);
  const timelineOpen = useStore((s) => !!s.timeline);
  // ▼ trilha C
  const compareOpen = useStore((s) => !!s.compare);
  const digestOpen = useStore((s) => s.digestSince !== null);
  useAwayFocus();
  // ▲ trilha C
  useEffect(() => {
    const s = useStore.getState();
    s.patch({ tabs: [wsId], active: wsId });
    s.loadPanes(wsId);
    s.loadMissions(wsId);
  }, [wsId]);
  return (
    <div className="app">
      <TopBar />
      <div className="app-body"><WorkspaceView wsId={wsId} hidden={false} /></div>
      {dialog === "painel" && <NewPaneDialog wsId={wsId} />}
      {dialog === "missao" && <NewMissionWizard wsId={wsId} />}
      {settingsOpen && <Suspense fallback={null}><Settings /></Suspense>}
      <CommandPalette />
      <ConversationExport />
      {reviewOpen && <Suspense fallback={null}><MissionReview /></Suspense>}
      {/* ▼ trilha B */}
      {timelineOpen && <Suspense fallback={null}><Timeline /></Suspense>}
      {/* ▲ trilha B */}
      {/* ▼ trilha C */}
      {compareOpen && <Suspense fallback={null}><CompareView /></Suspense>}
      {digestOpen && <Suspense fallback={null}><AwayDigest /></Suspense>}
      {/* ▲ trilha C */}
      <GitDialog />
      <VerifyDialog />
      <ModelSwitchDialog />
      <PaneSearch />
      <Toasts />
    </div>
  );
}

/** Janela principal e a de workspace (#/ws/<id>). */
export default function AppMain() {
  useAppEvents();
  useFileDrop();
  useVerifyToasts();
  // ▼ trilha B
  useCheckpointTrigger();
  // ▲ trilha B
  useAppShortcuts(true);
  const hash = window.location.hash;
  useEffect(() => startVisibilityReport(), []);
  useEffect(() => {
    // Só a janela principal abre o caminho recebido na linha de comando.
    useStore.getState().boot(!hash.startsWith("#/")).catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  }, [hash]);
  if (hash.startsWith("#/ws/")) return <WorkspaceWindow wsId={hash.slice(5)} />;
  return <MainWindow />;
}
