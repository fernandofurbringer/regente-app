import { lazy, Suspense, type ComponentProps } from "react";
import type { DiffEditor as DiffEditorType } from "@monaco-editor/react";

/** Monaco só entra quando um diff é aberto: o módulo de configuração (`../monaco`) carrega antes do editor. */
const DiffEditor = lazy(() => import("../monaco").then(() => import("@monaco-editor/react")).then((m) => ({ default: m.DiffEditor })));

/** Diff do Monaco carregado sob demanda. Reutilizável por qualquer tela que mostre diff. */
export default function LazyDiffEditor(props: ComponentProps<typeof DiffEditorType>) {
  return (
    <Suspense fallback={<div className="empty">Carregando editor…</div>}>
      <DiffEditor {...props} />
    </Suspense>
  );
}
