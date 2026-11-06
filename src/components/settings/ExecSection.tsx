import { errMsg, type ExecucoesPrefs, type Settings } from "../../lib/ipc";
import { useStore } from "../../state/store";

const clamp = (n: number, min: number, max: number, dflt: number) => Math.max(min, Math.min(max, Number.isFinite(n) ? Math.round(n) : dflt));

/** Execuções: o painel de comandos de shell de cada painel do Claude e a promoção a painel próprio. */
export default function ExecSection() {
  const settings = useStore((s) => s.settings) as Settings | null;
  if (!settings) return null;
  const prefs: ExecucoesPrefs = settings.execucoesPrefs;
  const save = (p: Partial<Settings>) => useStore.getState().saveSettings({ ...settings, ...p })
    .catch((e) => useStore.getState().toast(errMsg(e), "erro"));
  const savePrefs = (p: Partial<ExecucoesPrefs>) => save({ execucoesPrefs: { ...prefs, ...p } });
  const claudeOff = prefs.desligadoClis.includes("claude");

  return (
    <div className="stack" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
      <h2>Execuções e subagentes</h2>
      <p className="meta">
        Subagentes do Claude e comandos de shell aparecem na faixa de atividade do próprio painel (Ctrl+Shift+J abre a gaveta). Quem preferir pode dar a cada subagente, e à lista de comandos, um painel próprio na grade.
      </p>
      <label className="row"><input type="checkbox" checked={settings.execucoes} onChange={(e) => void save({ execucoes: e.target.checked })} /> Execuções ao vivo</label>
      <fieldset className="row" style={{ border: 0, padding: 0, gap: 12 }}>
        <legend className="meta">Subagentes</legend>
        <label className="row"><input type="radio" name="subagentes" checked={!settings.subagentPanes} onChange={() => void save({ subagentPanes: false })} /> Dentro do painel do Claude</label>
        <label className="row"><input type="radio" name="subagentes" checked={settings.subagentPanes} onChange={() => void save({ subagentPanes: true })} /> Painel próprio na grade</label>
      </fieldset>
      <label className="row"><input type="checkbox" checked={prefs.promover} onChange={(e) => void savePrefs({ promover: e.target.checked })} /> Comandos em painel próprio: a lista Execuções e os comandos longos (senão ficam na faixa do painel)</label>
      <label className="field">
        <span>Promover depois de (s) rodando</span>
        <input type="number" min={1} max={600} disabled={!prefs.promover} value={prefs.promoverAposS} onChange={(e) => void savePrefs({ promoverAposS: clamp(Number(e.target.value), 1, 600, 5) })} />
      </label>
      <label className="field">
        <span>Fechar o painel de um comando concluído com sucesso após (s); 0 mantém abertos</span>
        <input type="number" min={0} max={3600} disabled={!prefs.promover} value={prefs.fecharOkS} onChange={(e) => void savePrefs({ fecharOkS: clamp(Number(e.target.value), 0, 3600, 20) })} />
      </label>
      <label className="field">
        <span>Painéis de comando abertos ao mesmo tempo, por painel do Claude</span>
        <input type="number" min={1} max={20} disabled={!prefs.promover} value={prefs.maxPromovidos} onChange={(e) => void savePrefs({ maxPromovidos: clamp(Number(e.target.value), 1, 20, 6) })} />
      </label>
      <label className="row">
        <input type="checkbox" checked={claudeOff} onChange={(e) => void savePrefs({ desligadoClis: e.target.checked ? [...new Set([...prefs.desligadoClis, "claude"])] : prefs.desligadoClis.filter((c) => c !== "claude") })} />
        Desligar execuções para o Claude
      </label>
    </div>
  );
}
