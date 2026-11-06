import { lazy, Suspense, useEffect, type ReactNode } from "react";
import { api } from "./lib/ipc";

// cada janela carrega só o próprio código: o overlay do Shot não puxa Monaco, Configurações nem a janela principal
const AppMain = lazy(() => import("./AppMain"));
const PaneApp = lazy(() => import("./PaneApp"));
const ShotOverlay = lazy(() => import("./components/ShotOverlay"));
const ShotRecordBar = lazy(() => import("./components/ShotRecordBar"));
const RedlineWindow = lazy(() => import("./components/RedlineWindow"));
const ShotTrim = lazy(() => import("./components/ShotTrim"));
const ShotEditor = lazy(() => import("./components/ShotEditor"));

/** Janelas auxiliares do Shot e do Redline: cada uma carrega só a própria tela. */
function secondaryScreen(hash: string): ReactNode {
  if (hash.startsWith("#/shot-overlay")) return <ShotOverlay />;
  if (hash.startsWith("#/shot-record")) return <ShotRecordBar />;
  if (hash.startsWith("#/redline")) return <RedlineWindow />;
  if (hash.startsWith("#/shot-trim/")) return <ShotTrim path={decodeURIComponent(hash.slice("#/shot-trim/".length))} />;
  if (hash.startsWith("#/shot-editor/")) return <ShotEditor path={decodeURIComponent(hash.slice("#/shot-editor/".length))} />;
  return null;
}

export default function App() {
  const hash = window.location.hash;
  const secondary = secondaryScreen(hash);
  if (secondary) return <Suspense fallback={null}><SecondaryWindow>{secondary}</SecondaryWindow></Suspense>;
  const Root = hash.startsWith("#/pane/") ? PaneApp : AppMain;
  return <Suspense fallback={null}><Root /></Suspense>;
}

/** A secondary window skips the boot and the app events: it only needs the theme. */
function SecondaryWindow({ children }: { children: ReactNode }) {
  useEffect(() => {
    api.settingsGet().then((s) => { document.documentElement.dataset.theme = s.theme; }).catch(() => undefined);
  }, []);
  return <>{children}</>;
}
