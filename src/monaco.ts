import { loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/editor/editor.worker?worker";
import cssWorker from "monaco-editor/language/css/css.worker?worker";
import htmlWorker from "monaco-editor/language/html/html.worker?worker";
import jsonWorker from "monaco-editor/language/json/json.worker?worker";
import tsWorker from "monaco-editor/language/typescript/ts.worker?worker";

self.MonacoEnvironment = {
  getWorker(_: string, label: string) {
    if (label === "json") return new jsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new cssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new htmlWorker();
    if (label === "typescript" || label === "javascript") return new tsWorker();
    return new editorWorker();
  },
};

loader.config({ monaco });
// visualizador de agentes: sem validação semântica, senão "Cannot find module" aparece em todo import
// desde o monaco 0.57 o namespace é `monaco.typescript`; `languages.typescript` ficou undefined (quebrava Arquivos)
monaco.typescript.typescriptDefaults.setDiagnosticsOptions({ noSemanticValidation: true });
monaco.typescript.javascriptDefaults.setDiagnosticsOptions({ noSemanticValidation: true });

/** Tema escuro do editor (direção "Aurora"): fundo levemente translúcido para o brilho do app aparecer. */
export const TEMA_ESCURO = "regente-aurora";
monaco.editor.defineTheme(TEMA_ESCURO, {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "8a84a6", fontStyle: "italic" },
    { token: "keyword", foreground: "c5a3ff" },
    { token: "string", foreground: "f6b08a" },
    { token: "string.key.json", foreground: "b6c8ff" },
    { token: "string.value.json", foreground: "f6b08a" },
    { token: "number", foreground: "ffc08f" },
    { token: "regexp", foreground: "ff8fbf" },
    { token: "type", foreground: "6be0c8" },
    { token: "type.identifier", foreground: "6be0c8" },
    { token: "identifier", foreground: "dfe3ff" },
    { token: "delimiter", foreground: "b8b3cc" },
    { token: "tag", foreground: "ff8fbf" },
    { token: "attribute.name", foreground: "ffd98a" },
    { token: "attribute.value", foreground: "f6b08a" },
    { token: "variable", foreground: "b6c8ff" },
  ],
  colors: {
    "editor.background": "#0b0c16cc",
    "editor.foreground": "#f3f1fb",
    "editorGutter.background": "#0b0c16cc",
    "editor.lineHighlightBackground": "#b6a0ff12",
    "editor.lineHighlightBorder": "#00000000",
    "editor.selectionBackground": "#b6a0ff44",
    "editor.inactiveSelectionBackground": "#b6a0ff22",
    "editorLineNumber.foreground": "#7a7696",
    "editorLineNumber.activeForeground": "#d8d3ee",
    "editorCursor.foreground": "#cbbcff",
    "editorIndentGuide.background1": "#ffffff10",
    "editorIndentGuide.activeBackground1": "#b6a0ff55",
    "editorBracketHighlight.foreground1": "#ffd700",
    "editorBracketHighlight.foreground2": "#da70d6",
    "editorBracketHighlight.foreground3": "#4fc1ff",
    "editorWidget.background": "#141526",
    "editorSuggestWidget.background": "#141526",
    "editorHoverWidget.background": "#141526",
    "scrollbarSlider.background": "#b6a0ff22",
    "scrollbarSlider.hoverBackground": "#b6a0ff3a",
  },
});

export { monaco };
