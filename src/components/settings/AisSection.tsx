import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, errMsg, type CliDef, type CliVersion, type ProvidersConfig, type RouteDef } from "../../lib/ipc";
import { useStore } from "../../state/store";
import { spawnInActive } from "./Settings";
import InstallButton from "../InstallButton";
import { installSpec } from "../../lib/install";

export default function AisSection() {
  const providers = useStore((s) => s.providers);
  const detected = useStore((s) => s.detected);
  const settings = useStore((s) => s.settings);
  const [cfg, setCfg] = useState<ProvidersConfig | null>(providers);
  const [secrets, setSecrets] = useState<string[]>([]);
  const [newAcc, setNewAcc] = useState<Record<string, string>>({});
  const [keyInput, setKeyInput] = useState<Record<string, string>>({});
  // texto cru de "Modelos sugeridos" por rota: a vírgula digitada não some; vira lista no blur/salvar
  const [modelsRaw, setModelsRaw] = useState<Record<string, string>>({});
  const [versions, setVersions] = useState<Record<string, CliVersion>>({});
  const { toast, patch } = useStore.getState();

  useEffect(() => { api.vaultNames().then(setSecrets).catch(() => setSecrets([])); }, []);
  if (!cfg || !settings) return null;

  const path = (id: string) => detected.find((d) => d.kind === "cli" && d.id === id)?.path;
  const setCli = (i: number, p: Partial<CliDef>) => setCfg({ ...cfg, clis: cfg.clis.map((c, j) => (j === i ? { ...c, ...p } : c)) });
  const setRoute = (i: number, p: Partial<RouteDef>) => setCfg({ ...cfg, routes: cfg.routes.map((r, j) => (j === i ? { ...r, ...p } : r)) });
  const parseModels = (s: string) => s.split(",").map((x) => x.trim()).filter(Boolean);
  function commitModels(i: number, id: string) {
    if (!(id in modelsRaw)) return;
    setRoute(i, { models: parseModels(modelsRaw[id]) });
    const rest = { ...modelsRaw };
    delete rest[id];
    setModelsRaw(rest);
  }

  async function redetect() {
    patch({ detected: await api.providersDetect() });
  }
  async function save() {
    const next = { ...cfg!, routes: cfg!.routes.map((r) => (r.id in modelsRaw ? { ...r, models: parseModels(modelsRaw[r.id]) } : r)) };
    try {
      await api.providersSave(next);
      setCfg(next);
      setModelsRaw({});
      patch({ providers: next });
      await redetect();
      toast("Salvo", "ok");
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function addAccount(cli: string) {
    const n = (newAcc[cli] ?? "").trim();
    if (!n) return;
    try {
      patch({ settings: await api.accountAdd(cli, n) });
      setNewAcc({ ...newAcc, [cli]: "" });
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function setKey(r: RouteDef) {
    const v = keyInput[r.id]?.trim();
    if (!v || !r.secret) return;
    try {
      await api.vaultSet(r.secret, v);
      setSecrets(await api.vaultNames());
      setKeyInput({ ...keyInput, [r.id]: "" });
      toast("Chave salva no Cofre", "ok");
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  /** Proxy HTTPS de uma conta extra: vazio remove. A URL é validada no backend. */
  async function saveProxy(cli: string, a: string, url: string) {
    const s = useStore.getState().settings;
    if (!s) return;
    const key = `${cli}/${a}`;
    const next = { ...s, accountProxy: { ...s.accountProxy, [key]: url.trim() } };
    try {
      await api.settingsSet(next);
      patch({ settings: next });
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function removeAccount(cli: string, a: string) {
    if (!(await ask(`Remover a conta "${a}"? O login dela neste computador não é apagado.`, { title: "Remover conta", kind: "warning" }))) return;
    try {
      patch({ settings: await api.accountRemove(cli, a) });
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function checkVersion(id: string) {
    try {
      const r = await api.cliVersionCheck(id);
      setVersions((vs) => ({ ...vs, [id]: r }));
    } catch (e) {
      toast(errMsg(e), "erro");
    }
  }
  async function test(r: RouteDef) {
    try {
      toast(`${r.label}: ${await api.routeTest(r.id)}`, "ok");
    } catch (e) {
      toast(`${r.label}: ${errMsg(e)}`, "erro");
    }
  }

  return (
    <>
      <h3>Suas IAs</h3>
      <div className="row" style={{ marginBottom: 8 }}>
        <button className="btn" onClick={() => redetect().then(() => toast("Detecção atualizada", "ok"), (e) => toast(errMsg(e), "erro"))}>Detectar de novo</button>
        <button className="btn primary" onClick={save}>Salvar alterações</button>
      </div>
      {cfg.clis.map((c, i) => (
        <div key={c.id} className="card">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <strong>{c.label}</strong>
            <span className="chip">{path(c.id) ? `instalado: ${path(c.id)}` : "não encontrado"}</span>
          </div>
          <div className="row" style={{ alignItems: "flex-end" }}>
            <div className="field" style={{ flex: 1 }}><span>Comando</span><input value={c.command} onChange={(e) => setCli(i, { command: e.target.value })} /></div>
            {!path(c.id) && c.install && <InstallButton label={c.label} command={c.install} onConfirm={() => spawnInActive(installSpec(c.label, c.install!))} />}
          </div>
          <div className="row" style={{ alignItems: "center" }}>
            <button className="btn" onClick={() => checkVersion(c.id)}>Verificar versão</button>
            {versions[c.id] && (() => {
              const v = versions[c.id];
              return (
                <>
                  <span className="chip">{`instalada: ${v.current ?? "n/d"} · última: ${v.latest ?? "n/d"}`}</span>
                  {v.note && <span className="muted">{v.note}</span>}
                  {v.newer && v.updateCmd && (
                    <button className="btn primary" onClick={() => spawnInActive({ kind: "terminal", name: `Atualizar ${c.label}`, run: v.updateCmd! })}>Atualizar</button>
                  )}
                </>
              );
            })()}
          </div>
          {c.accountEnv && (
            <div className="field">
              <span>Contas (a "principal" é o login que já está no computador)</span>
              <div className="row">
                <span className="chip">principal</span>
                {(settings.accounts[c.id] ?? []).map((a) => (
                  <span key={a} className="chip">
                    {a}
                    <button className="icon-btn" title="Abrir um painel com esta conta para fazer o login" onClick={() => spawnInActive({ kind: "agent", cli: c.id, account: a, name: `Login ${c.label} (${a})` })}>entrar</button>
                    <button className="icon-btn" aria-label={`Remover conta ${a}`} onClick={() => removeAccount(c.id, a)}>✕</button>
                  </span>
                ))}
              </div>
              {(settings.accounts[c.id] ?? []).map((a) => (
                <div className="row" key={`proxy-${a}`}>
                  <input style={{ width: 280 }} placeholder="http://proxy:3128 (opcional)" aria-label={`Proxy HTTPS da conta ${a}`}
                    defaultValue={settings.accountProxy?.[`${c.id}/${a}`] ?? ""} key={`px-${c.id}-${a}-${settings.accountProxy?.[`${c.id}/${a}`] ?? ""}`}
                    onBlur={(e) => saveProxy(c.id, a, e.target.value)} />
                  <span className="meta">Proxy HTTPS só dos painéis desta conta (sem usuário e senha na URL).</span>
                </div>
              ))}
              <div className="row">
                <input style={{ width: 130 }} placeholder="nova conta" aria-label={`Nova conta ${c.label}`} value={newAcc[c.id] ?? ""}
                  onChange={(e) => setNewAcc({ ...newAcc, [c.id]: e.target.value })} onKeyDown={(e) => { if (e.key === "Enter") addAccount(c.id); }} />
                <button className="btn" onClick={() => addAccount(c.id)}>Adicionar</button>
              </div>
            </div>
          )}
        </div>
      ))}
      <h3>Rotas por chave (Claude Code)</h3>
      {cfg.routes.map((r, i) => (
        <div key={r.id} className="card">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <strong>{r.label}</strong>
            {r.secret && <span className="chip">{secrets.includes(r.secret) ? "chave configurada" : "sem chave"}</span>}
          </div>
          <div className="field"><span>Endereço</span><input value={r.baseUrl} onChange={(e) => setRoute(i, { baseUrl: e.target.value })} /></div>
          <div className="field">
            <span>Modelos sugeridos (separados por vírgula)</span>
            <input value={modelsRaw[r.id] ?? r.models.join(", ")} onChange={(e) => setModelsRaw({ ...modelsRaw, [r.id]: e.target.value })} onBlur={() => commitModels(i, r.id)} />
          </div>
          <div className="row">
            {r.secret && (
              <>
                <input type="password" placeholder="cole a chave da API" aria-label={`Chave ${r.label}`} value={keyInput[r.id] ?? ""} onChange={(e) => setKeyInput({ ...keyInput, [r.id]: e.target.value })} />
                <button className="btn" onClick={() => setKey(r)}>Salvar chave</button>
              </>
            )}
            <button className="btn" onClick={() => test(r)}>Testar conexão</button>
          </div>
        </div>
      ))}
    </>
  );
}
