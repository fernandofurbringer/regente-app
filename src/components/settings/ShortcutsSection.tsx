import { useEffect, useState } from "react";
import { api, errMsg, type ShotConfig } from "../../lib/ipc";
import { formatHotkey, type KeyLike } from "../../lib/hotkey";
import { findConflict, resolveShortcuts, SHORTCUTS as DEFS, type GlobalCombo } from "../../lib/shortcuts";
import { useStore } from "../../state/store";

/** Atalhos do terminal e do editor: fixos, não entram na lista editável. */
const FIXED: [string, string][] = [
  ["Ctrl+F", "Buscar no terminal (Enter próximo, Shift+Enter anterior, Esc fecha)"],
  ["Ctrl+C", "Copiar a seleção do terminal (sem seleção, Ctrl+C envia Ctrl+C ao programa)"],
  ["Ctrl+V / Ctrl+Shift+V", "Colar no terminal"],
  ["Enter / Esc", "Confirmar / cancelar o renomear"],
  ["Duplo clique no cabeçalho", "Focar o painel (isolar na grade)"],
  ["Ctrl+S", "Salvar no editor"],
  ["Setas, Home, End", "Navegar na árvore de arquivos (→ expande, ← recolhe ou sobe)"],
  ["F2 / Del", "Renomear / apagar o item selecionado na árvore"],
];

/** Atalhos da janela editáveis. Ficam salvos em settings.json (`shortcuts`); conflitos com os globais são recusados. */
export default function ShortcutsSection() {
  const settings = useStore((s) => s.settings);
  const voice = useStore((s) => s.voice);
  const [shot, setShot] = useState<ShotConfig | null>(null);
  const [recording, setRecording] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const resolved = resolveShortcuts(settings?.shortcuts);

  useEffect(() => {
    api.shotGet().then((r) => setShot(r.config)).catch(() => {});
  }, []);

  const globals: GlobalCombo[] = [
    ...(voice ? [{ label: "o ditado (tecla do microfone)", combo: voice.hotkey }] : []),
    ...(voice?.globalEnabled ? [{ label: "o ditado global", combo: voice.globalHotkey }] : []),
    ...(shot ? [
      { label: "a captura da tela inteira", combo: shot.fullKey },
      { label: "a captura de região", combo: shot.regionKey },
      { label: "a gravação de região", combo: shot.recordKey },
    ] : []),
  ];

  async function save(shortcuts: Record<string, string>) {
    const st = useStore.getState();
    if (!st.settings) return;
    try {
      await st.saveSettings({ ...st.settings, shortcuts });
    } catch (e) {
      st.toast(errMsg(e), "erro");
    }
  }

  // grava a próxima combinação; Esc cancela, só modificador é ignorado
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.code === "Escape") {
        setRecording(null);
        setErr(null);
        return;
      }
      const spec = formatHotkey(e as KeyLike);
      if (!spec) return;
      const bad = findConflict(recording, spec, resolved, globals);
      if (bad) {
        setErr(bad);
        return;
      }
      setRecording(null);
      setErr(null);
      void save({ ...(settings?.shortcuts ?? {}), [recording]: spec });
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recording, shot, voice, settings]);


  return (
    <>
      <h3>Atalhos</h3>
      <p className="meta">Atalhos da janela do Regente (não são globais). Clique em "alterar" e pressione a nova combinação; Esc cancela.</p>
      <p className="meta">Dentro do terminal, Ctrl+letra vai para o programa (ex.: Ctrl+T do Claude Code). Use Ctrl+Shift+P para a paleta.</p>
      <table className="tbl"><tbody>
        {DEFS.map((d) => (
          <tr key={d.id}>
            <td>{d.label}</td>
            <td><span className="kbd">{resolved[d.id]}</span></td>
            <td>
              {recording === d.id
                ? <span role="status">Pressione a combinação…</span>
                : <button className="btn" aria-label={`Alterar atalho: ${d.label}`} onClick={() => { setErr(null); setRecording(d.id); }}>alterar</button>}
            </td>
          </tr>
        ))}
      </tbody></table>
      {err && <div className="meta" role="alert" style={{ color: "var(--danger, #f85149)" }}>{err}</div>}
      <div className="actions"><button className="btn" onClick={() => { setErr(null); void save({}); }}>restaurar padrões</button></div>
      <h3>Fixos do terminal e do editor</h3>
      <table className="tbl"><tbody>{FIXED.map(([k, d]) => <tr key={k}><td><span className="kbd">{k}</span></td><td>{d}</td></tr>)}</tbody></table>
    </>
  );
}
