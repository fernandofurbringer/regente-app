import { type Settings } from "../../lib/ipc";
import { useStore } from "../../state/store";

export default function NotifySection() {
  const settings = useStore((s) => s.settings);
  if (!settings) return null;
  const save = (p: Partial<Settings>) => useStore.getState().saveSettings({ ...settings, ...p });
  return (
    <>
      <h3>Notificações</h3>
      <div className="card">
        <strong>Avisos do Windows</strong>
        <small style={{ color: "var(--fg3)" }}>Só aparecem com o Regente fora de foco ou com outro painel na frente; no máximo 1 por painel a cada 30 s.</small>
        <label className="row"><input type="checkbox" checked={settings.notify.finished} onChange={(e) => save({ notify: { ...settings.notify, finished: e.target.checked } })} /> Notificar quando um agente terminar (trabalhos de 20 s ou mais)</label>
        <label className="row"><input type="checkbox" checked={settings.notify.asks} onChange={(e) => save({ notify: { ...settings.notify, asks: e.target.checked } })} /> Notificar quando um agente perguntar</label>
        <label className="row"><input type="checkbox" checked={settings.notify.gates} onChange={(e) => save({ notify: { ...settings.notify, gates: e.target.checked } })} /> Notificar quando abrir um gate</label>
      </div>
    </>
  );
}
