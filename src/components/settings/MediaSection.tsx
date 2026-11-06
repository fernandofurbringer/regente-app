import { useEffect, useState } from "react";
import { api, errMsg, type MediaConfig } from "../../lib/ipc";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const KINDS: [string, string][] = [
  ["google-image", "Imagem · Gemini"],
  ["openai-image", "Imagem · GPT Image"],
  ["replicate-video", "Vídeo · Replicate"],
  ["elevenlabs", "Áudio · ElevenLabs"],
];
const PROVIDERS: [string, string][] = [
  ["google", "Google (Gemini)"],
  ["openai", "OpenAI"],
  ["replicate", "Replicate"],
  ["elevenlabs", "ElevenLabs"],
];
/** Tipos com teto de gasto (os chamados pelo MCP), com o nome que aparece na tela. */
const CAPS: [string, string][] = [
  ["image", "Imagens"],
  ["video", "Vídeos"],
  ["audio", "Áudios"],
];

type Usage = { kind: string; today: number; perDay: number | null; perMission: number | null };

/** Settings → Mídia: provedores, modelos, voz, chaves no cofre, tetos de gasto e [Testar] por provedor. */
export default function MediaSection() {
  const [cfg, setCfg] = useState<MediaConfig | null>(null);
  const [usage, setUsage] = useState<Usage[]>([]);
  const [vozLocal, setVozLocal] = useState(false);
  useEffect(() => {
    api.vozLocalStatus().then((s) => setVozLocal(s.instalada)).catch(() => setVozLocal(false));
  }, []);
  const [vault, setVault] = useState<string[]>([]);
  const [keys, setKeys] = useState<Record<string, string>>({});
  const [testing, setTesting] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, string>>({});

  const refreshUsage = () => api.mediaUsage().then(setUsage).catch(toast);

  useEffect(() => {
    api.mediaGet().then(setCfg).catch(toast);
    api.vaultNames().then(setVault).catch(toast);
    refreshUsage();
  }, []);

  if (!cfg) return <><h3>Mídia</h3><div className="meta">Carregando…</div></>;

  const set = (p: Partial<MediaConfig>) => setCfg({ ...cfg, ...p });
  const toggle = (kind: string, on: boolean) => set({ enabled: on ? [...cfg.enabled, kind] : cfg.enabled.filter((k) => k !== kind) });
  /** Teto inteiro >= 0. Campo vazio ou inválido é ignorado (o valor anterior continua). */
  const setCap = (map: "maxPerDay" | "maxPerMission", kind: string, raw: string) => {
    const n = Math.floor(Number(raw));
    if (raw.trim() === "" || !Number.isFinite(n) || n < 0) return;
    set({ [map]: { ...cfg[map], [kind]: n } } as Partial<MediaConfig>);
  };

  async function save() {
    try {
      await api.mediaSave(cfg!);
      useStore.getState().toast("Mídia salva", "ok");
      refreshUsage();
    } catch (e) {
      toast(e);
    }
  }

  async function saveKey(p: string) {
    try {
      await api.vaultSet(`media.${p}`, keys[p] ?? "");
      setKeys({ ...keys, [p]: "" });
      setVault(await api.vaultNames());
      useStore.getState().toast("Chave salva no Cofre", "ok");
    } catch (e) {
      toast(e);
    }
  }

  async function test(p: string) {
    setTesting(p);
    try {
      const r = await api.mediaTest(p);
      setResults({ ...results, [p]: r });
    } catch (e) {
      setResults({ ...results, [p]: errMsg(e) });
    } finally {
      setTesting(null);
    }
  }

  return (
    <>
      <h3>Mídia</h3>
      <p style={{ color: "var(--fg3)" }}>Imagem, vídeo e áudio gerados por IA. Cada geração é paga pelo provedor e grava o arquivo em <code>assets</code> da pasta do painel.</p>

      <h4>Provedores habilitados</h4>
      {KINDS.map(([k, label]) => (
        <label key={k} className="row"><input type="checkbox" checked={cfg.enabled.includes(k)} onChange={(e) => toggle(k, e.target.checked)} /> {label}</label>
      ))}

      <h4>Modelos</h4>
      <div className="row">
        <label className="field"><span>Provedor de imagem padrão</span>
          <select value={cfg.imageProvider} onChange={(e) => set({ imageProvider: e.target.value as MediaConfig["imageProvider"] })}>
            <option value="google">Google (Gemini)</option>
            <option value="openai">OpenAI (GPT Image)</option>
          </select>
        </label>
        <label className="field"><span>Modelo Gemini</span><input value={cfg.googleModel} onChange={(e) => set({ googleModel: e.target.value })} /></label>
        <label className="field"><span>Modelo GPT Image</span><input value={cfg.openaiModel} onChange={(e) => set({ openaiModel: e.target.value })} /></label>
      </div>
      <div className="row">
        <label className="field"><span>Modelo de vídeo (Replicate, owner/nome)</span><input value={cfg.videoModel} onChange={(e) => set({ videoModel: e.target.value })} /></label>
        <label className="field"><span>Voz ElevenLabs (id)</span><input value={cfg.elevenVoice} onChange={(e) => set({ elevenVoice: e.target.value })} /></label>
      </div>
      <p className="meta">Voz: local (grátis) se instalada; ElevenLabs para efeitos.{vozLocal ? "" : " A voz local ainda não está instalada: Configurações → Voz."}</p>

      <h4>Limites de gasto (agente pelo MCP)</h4>
      <p className="meta">Contam as tentativas, inclusive as que falharam. A tela (ou [Testar]) não conta. Os limites valem para todas as missões, por dia (desde a meia-noite) e por missão.</p>
      {CAPS.map(([k, label]) => {
        const u = usage.find((x) => x.kind === k);
        return (
          <div key={k} className="row">
            <span style={{ minWidth: 120 }}>{label}</span>
            <span className="meta">hoje {u?.today ?? 0}</span>
            <label className="field"><span>por dia</span>
              <input type="number" min={0} aria-label={`Limite diário de ${label.toLowerCase()}`} value={cfg.maxPerDay[k] ?? ""} onChange={(e) => setCap("maxPerDay", k, e.target.value)} />
            </label>
            <label className="field"><span>por missão</span>
              <input type="number" min={0} aria-label={`Limite por missão de ${label.toLowerCase()}`} value={cfg.maxPerMission[k] ?? ""} onChange={(e) => setCap("maxPerMission", k, e.target.value)} />
            </label>
          </div>
        );
      })}

      <h4>Chaves (Cofre)</h4>
      {PROVIDERS.map(([p, label]) => (
        <div key={p} className="row">
          <span style={{ minWidth: 160 }}>{label}</span>
          <input type="password" placeholder="valor da chave" aria-label={`Chave do ${label}`} value={keys[p] ?? ""} onChange={(e) => setKeys({ ...keys, [p]: e.target.value })} />
          <button className="btn" disabled={!keys[p]} onClick={() => saveKey(p)}>Salvar chave</button>
          <span className="meta">{vault.includes(`media.${p}`) ? "chave salva no Cofre" : "sem chave no Cofre"}</span>
          <button className="btn" disabled={testing !== null} onClick={() => test(p)}>{testing === p ? "Testando…" : "Testar"}</button>
          {results[p] && <span className="meta">{results[p]}</span>}
        </div>
      ))}
      <p className="meta">[Testar] confere a chave com uma leitura gratuita da conta do provedor (lista de modelos ou dados da conta). Não gera nada e não gasta créditos.</p>

      <div className="row" style={{ marginTop: 10 }}>
        <span className="spacer" />
        <button className="btn primary" onClick={save}>Salvar</button>
      </div>
    </>
  );
}
