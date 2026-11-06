import { useEffect, useState } from "react";
import { api, errMsg, type JarvisConfig } from "../../lib/ipc";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const PROVIDERS: Record<JarvisConfig["provider"], string> = { anthropic: "Anthropic", openai: "Compatível com OpenAI" };
const EFFORTS: Record<JarvisConfig["effort"], string> = { low: "Baixo (mais rápido)", medium: "Médio", high: "Alto" };

export default function IaAuxiliarSection() {
  const [cfg, setCfg] = useState<JarvisConfig | null>(null);
  const [vault, setVault] = useState<string[]>([]);
  const [secretValue, setSecretValue] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.jarvisGet().then(setCfg).catch(toast);
    api.vaultNames().then(setVault).catch(toast);
  }, []);

  if (!cfg) return <><h3>IA auxiliar</h3><div className="meta">Carregando…</div></>;

  const set = (p: Partial<JarvisConfig>) => setCfg({ ...cfg, ...p });
  const keyName = cfg.secret ?? (cfg.provider === "openai" ? "jarvis.openai" : "jarvis.anthropic");

  async function save() {
    if (!cfg || busy) return;
    setBusy(true);
    try {
      await api.jarvisSave(cfg);
      setCfg(await api.jarvisGet());
      useStore.getState().toast("IA auxiliar salva", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
    }
  }

  async function saveKey() {
    try {
      await api.vaultSet(keyName, secretValue);
      setSecretValue("");
      setVault(await api.vaultNames());
      useStore.getState().toast("Chave salva no Cofre", "ok");
    } catch (e) {
      toast(e);
    }
  }

  return (
    <>
      <h3>IA auxiliar</h3>
      <p style={{ color: "var(--fg3)" }}>Um modelo de texto para as sugestões do Regente: mensagem de commit, "Me sugere" do briefing, sugestão de instruções do projeto, resumo de quando você esteve fora e o ditado em inglês no modo código. Sem chave, essas sugestões ficam desligadas.</p>

      <label className="field"><span>Provedor</span>
        <select value={cfg.provider} onChange={(e) => set({ provider: e.target.value as JarvisConfig["provider"] })}>
          {Object.entries(PROVIDERS).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
        </select>
      </label>
      <label className="field"><span>URL base</span>
        <input value={cfg.baseUrl} placeholder={cfg.provider === "openai" ? "https://api.openai.com/v1" : "https://api.anthropic.com"} onChange={(e) => set({ baseUrl: e.target.value })} />
      </label>
      <div className="row">
        <label className="field"><span>Modelo</span><input value={cfg.model} onChange={(e) => set({ model: e.target.value })} /></label>
        <label className="field"><span>Esforço</span>
          <select value={cfg.effort} onChange={(e) => set({ effort: e.target.value as JarvisConfig["effort"] })}>
            {Object.entries(EFFORTS).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
        </label>
      </div>

      <label className="field"><span>Nome da chave no Cofre</span>
        <input value={cfg.secret ?? ""} placeholder={keyName} onChange={(e) => set({ secret: e.target.value || null })} />
      </label>
      <div className="row">
        <input type="password" placeholder="valor da chave" aria-label="Chave da IA auxiliar" value={secretValue} onChange={(e) => setSecretValue(e.target.value)} />
        <button className="btn" disabled={!secretValue} onClick={saveKey}>Salvar chave</button>
        <span className="meta">{vault.includes(keyName) ? "chave salva no Cofre" : "sem chave no Cofre"}</span>
      </div>

      <div className="row" style={{ marginTop: 10 }}>
        <span className="spacer" />
        <button className="btn primary" disabled={busy} onClick={save}>Salvar</button>
      </div>
    </>
  );
}
