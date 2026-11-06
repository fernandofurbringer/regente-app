import { useEffect, useState } from "react";
import { api, errMsg, type Settings } from "../../lib/ipc";
import { useStore } from "../../state/store";

export default function GeneralSection() {
  const settings = useStore((s) => s.settings);
  const providers = useStore((s) => s.providers);
  const [info, setInfo] = useState<{ dataDir: string; version: string } | null>(null);
  // rascunhos de fonte e tamanho: salvam no blur/Enter, não a cada tecla (null = sem edição)
  const [font, setFont] = useState<string | null>(null);
  const [size, setSize] = useState<string | null>(null);
  const [scroll, setScroll] = useState<string | null>(null);
  useEffect(() => { api.appInfo().then(setInfo).catch(() => {}); }, []);
  if (!settings || !providers) return null;
  const save = (p: Partial<Settings>) => useStore.getState().saveSettings({ ...settings, ...p })
    .then(() => true, (e) => { useStore.getState().toast(errMsg(e), "erro"); return false; });
  function commitFont() {
    const f = font?.trim();
    setFont(null);
    if (f && f !== settings!.terminalFont) save({ terminalFont: f });
  }
  async function useWtFont() {
    try {
      const stack = await api.terminalFontFromWt();
      setFont(null);
      if (await save({ terminalFont: stack })) useStore.getState().toast("Fonte do Windows Terminal aplicada", "ok");
    } catch (e) {
      useStore.getState().toast(errMsg(e), "erro");
    }
  }
  function commitSize() {
    if (size === null) return;
    setSize(null);
    const n = Math.max(9, Math.min(24, Math.round(Number(size)) || 13));
    if (n !== settings!.terminalFontSize) save({ terminalFontSize: n });
  }
  function commitScroll() {
    if (scroll === null) return;
    setScroll(null);
    // mesma faixa do backend (settings::clamp_scrollback): o servidor corrige o que escapar
    const n = Math.max(1000, Math.min(20000, Math.round(Number(scroll)) || 5000));
    if (n !== settings!.scrollback) save({ scrollback: n });
  }
  return (
    <>
      <h3>Aparência</h3>
      <label className="field">
        <span>Tema</span>
        <select value={settings.theme} onChange={(e) => save({ theme: e.target.value as Settings["theme"] })}>
          <option value="dark">Escuro</option>
          <option value="light">Claro</option>
          <option value="contraste">Alto contraste (escuro, bordas brancas)</option>
        </select>
      </label>
      <label className="field">
        <span>Fonte do terminal</span>
        <input value={font ?? settings.terminalFont} onChange={(e) => setFont(e.target.value)} onBlur={commitFont} onKeyDown={(e) => { if (e.key === "Enter") commitFont(); }} />
      </label>
      <p className="meta">Para ícones do oh-my-posh/Starship use uma Nerd Font (ex.: FiraCode Nerd Font)</p>
      <button className="btn" onClick={useWtFont}>Usar a fonte do Windows Terminal</button>
      <label className="field">
        <span>Tamanho da fonte (9 a 24)</span>
        <input type="number" min={9} max={24} value={size ?? settings.terminalFontSize} onChange={(e) => setSize(e.target.value)} onBlur={commitSize}
          onKeyDown={(e) => { if (e.key === "Enter") commitSize(); }} />
      </label>
      <label className="field">
        <span>Linhas de histórico por terminal (1000 a 20000)</span>
        <input type="number" min={1000} max={20000} step={500} value={scroll ?? settings.scrollback} onChange={(e) => setScroll(e.target.value)} onBlur={commitScroll}
          onKeyDown={(e) => { if (e.key === "Enter") commitScroll(); }} />
      </label>
      <label className="field">
        <span>Shell padrão do + TERMINAL</span>
        <select value={settings.defaultShell} onChange={(e) => save({ defaultShell: e.target.value })}>
          {providers.shells.map((s) => <option key={s.id} value={s.id}>{s.label}</option>)}
        </select>
      </label>
      <label className="field">
        <span>Terminais abrindo ao mesmo tempo (missões)</span>
        <input type="number" min={1} max={16} value={settings.spawnConcurrency} onChange={(e) => save({ spawnConcurrency: Math.max(1, Math.min(16, Number(e.target.value) || 4)) })} />
      </label>
      <label className="row"><input type="checkbox" checked={settings.allowRegenteTools} onChange={(e) => save({ allowRegenteTools: e.target.checked })} /> Permitir as ferramentas do Regente sem perguntar</label>
      <label className="row"><input type="checkbox" checked={settings.restoreOnOpen} onChange={(e) => save({ restoreOnOpen: e.target.checked })} /> Restaurar a sessão ao abrir (sem perguntar)</label>
      <label className="row"><input type="checkbox" checked={settings.screenReaderMode} onChange={(e) => save({ screenReaderMode: e.target.checked })} /> Modo leitor de tela no terminal (mais lento; o leitor lê o buffer)</label>
      {/* ▼ trilha B */}
      <label className="row"><input type="checkbox" checked={settings.checkpointsRaiz} onChange={(e) => save({ checkpointsRaiz: e.target.checked })} /> Checkpoints também na raiz do workspace (se for repositório git)</label>
      {/* ▲ trilha B */}
      <label className="field">
        <span>Fechar subagentes concluídos após (s), só no modo painel próprio; 0 mantém abertos</span>
        <input type="number" min={0} max={3600} value={settings.subagentCloseSecs} onChange={(e) => save({ subagentCloseSecs: Math.max(0, Math.min(3600, Number(e.target.value) || 0)) })} />
      </label>
      <div className="card">
        <strong>Onboarding</strong>
        <small style={{ display: "block", color: "var(--fg3)" }}>{settings.onboardingDone ? "Concluído." : "Pendente: o assistente de boas-vindas aparece até ser concluído."}</small>
        <div className="actions">
          <button className="btn" disabled={!settings.onboardingDone} onClick={() => useStore.getState().patch({ onboardingOpen: true, settingsOpen: false })}>Retomar onboarding</button>
          <button className="btn" disabled={settings.onboardingDone} onClick={() => save({ onboardingDone: true })}>Considerar concluído</button>
          <button className="btn" onClick={() => save({ onboardingDone: false }).then((ok) => { if (ok) useStore.getState().patch({ settingsOpen: false }); })}>Reiniciar onboarding</button>
        </div>
      </div>
      <div className="card">
        <strong>Sobre</strong>
        <div>Regente {info?.version ?? "…"}</div>
        <small style={{ color: "var(--fg3)" }}>Dados em {info?.dataDir ?? "…"}</small>
      </div>
    </>
  );
}
