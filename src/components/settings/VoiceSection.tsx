import { useEffect, useRef, useState } from "react";
import { formatHotkey } from "../../lib/hotkey";
import { api, errCode, errMsg, type Settings, type VoiceConfig, type VoiceEntry, type VozLocalStatus, type WhisperLocalStatus, type WhisperModelo } from "../../lib/ipc";
import { micMessage, VoiceRecorder } from "../../lib/recorder";
import { playAudio } from "../../lib/speech";
import { useStore } from "../../state/store";

const toast = (e: unknown) => useStore.getState().toast(errMsg(e), "erro");
const ENGINES: Record<VoiceConfig["engine"], string> = { groq: "Groq (nuvem)", openai: "Compatível com OpenAI", local: "Local (Whisper no computador)" };
const MODES: Record<VoiceConfig["mode"], string> = { codigo: "Código", planejamento: "Planejamento", conversa: "Conversa" };
const LANGS: Record<VoiceConfig["language"], string> = { auto: "Automático", pt: "Português", en: "Inglês", es: "Espanhol" };
const parseTerms = (text: string) => text.split(",").map((t) => t.trim()).filter(Boolean);
const VOZES_LOCAIS: [string, string][] = [
  ["F1", "Feminina 1"], ["F2", "Feminina 2"], ["F3", "Feminina 3"], ["F4", "Feminina 4"], ["F5", "Feminina 5"],
  ["M1", "Masculina 1"], ["M2", "Masculina 2"], ["M3", "Masculina 3"], ["M4", "Masculina 4"], ["M5", "Masculina 5"],
];
const QUALIDADES: [number, string][] = [[4, "Rápida"], [8, "Normal"], [16, "Alta"]];

/** Cartão da voz local (Supertonic 3): baixar (com aviso de licença), ouvir, ajustar e remover. Ajustes salvos com atraso. */
function VozLocal() {
  const settings = useStore((s) => s.settings);
  const [st, setSt] = useState<VozLocalStatus | null>(null);
  const [prog, setProg] = useState<{ nome: string; bytes: number; total: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [prefs, setPrefs] = useState<Settings["vozLocal"] | null>(null);
  /** Só salva quando o usuário mexeu (não ao abrir a tela). */
  const mudou = useRef(false);

  const recarregar = () => api.vozLocalStatus().then(setSt).catch(toast);
  useEffect(() => {
    recarregar();
  }, []);

  useEffect(() => {
    if (!mudou.current || !prefs) return;
    const t = window.setTimeout(() => {
      const s = useStore.getState().settings;
      if (s) useStore.getState().saveSettings({ ...s, vozLocal: prefs }).catch(toast);
    }, 400);
    return () => window.clearTimeout(t);
  }, [prefs]);

  const p = prefs ?? settings?.vozLocal;
  if (!p || !st) return null;
  const voz = p.voz;
  const ajustar = (q: Partial<Settings["vozLocal"]>) => {
    mudou.current = true;
    setPrefs({ ...p, ...q });
  };

  async function instalar() {
    setBusy(true);
    try {
      await api.vozLocalInstalar((e) => {
        if (e.evento === "arquivo") setProg({ nome: e.nome, bytes: e.bytes, total: e.total });
      });
      useStore.getState().toast("Voz local instalada", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
      setProg(null);
      recarregar();
    }
  }

  async function remover() {
    if (!window.confirm("Remover os arquivos da voz local? Você pode baixar de novo depois.")) return;
    setBusy(true);
    try {
      await api.vozLocalRemover();
      useStore.getState().toast("Voz local removida", "ok");
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
      recarregar();
    }
  }

  async function ouvir() {
    try {
      await playAudio("audio/wav", await api.vozLocalOuvir(voz));
    } catch (e) {
      toast(e);
    }
  }

  return (
    <>
      <h4>Voz local (Supertonic 3)</h4>
      {!st.instalada ? (
        <>
          <p className="meta">Voz em português que roda no seu computador, sem chave e sem custo. Baixa ~400 MB uma vez.</p>
          <p className="meta">Modelo Supertonic 3 (licença OpenRAIL-M: não use para se passar por outra pessoa nem para conteúdo enganoso). Código de inferência adaptado do exemplo MIT da Supertone.</p>
          <div className="row">
            <button className="btn primary" disabled={busy} onClick={instalar}>Baixar voz local</button>
            {busy && <button className="btn" onClick={() => api.vozLocalCancelar().catch(toast)}>Cancelar</button>}
          </div>
          {busy && prog && (
            <div className="row">
              <span className="meta">{prog.nome} · {Math.round((prog.bytes / Math.max(prog.total, 1)) * 100)}%</span>
              <span className="lvl-bar" role="meter" aria-label="Progresso do arquivo" aria-valuenow={Math.round((prog.bytes / Math.max(prog.total, 1)) * 100)}>
                <i style={{ width: `${(prog.bytes / Math.max(prog.total, 1)) * 100}%` }} />
              </span>
            </div>
          )}
        </>
      ) : (
        <>
          <div className="row">
            <label className="field"><span>Voz</span>
              <select value={p.voz} onChange={(e) => ajustar({ voz: e.target.value })}>
                {VOZES_LOCAIS.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
              </select>
            </label>
            <button className="btn" onClick={ouvir}>Ouvir</button>
          </div>
          <div className="row">
            <label className="field"><span>Velocidade ({p.velocidade.toFixed(2).replace(".", ",")})</span>
              <input type="range" min={0.8} max={1.3} step={0.05} value={p.velocidade} aria-label="Velocidade da voz local" onChange={(e) => ajustar({ velocidade: Number(e.target.value) })} />
            </label>
            <label className="field"><span>Qualidade</span>
              <select value={p.passos} onChange={(e) => ajustar({ passos: Number(e.target.value) })}>
                {QUALIDADES.map(([n, l]) => <option key={n} value={n}>{l}</option>)}
              </select>
            </label>
          </div>
          <div className="row">
            <button className="btn" disabled={busy} onClick={remover}>Remover arquivos baixados</button>
          </div>
        </>
      )}
    </>
  );
}

const MODELOS_WHISPER: { id: WhisperModelo; titulo: string; nota: string }[] = [
  { id: "leve", titulo: "Rápido e leve (small, 190 MB) · recomendado", nota: "Cerca de 5 s por ditado curto num notebook sem placa de vídeo dedicada. Usa ~0,85 GB de memória enquanto ativo." },
  { id: "preciso", titulo: "Preciso (large-v3-turbo, 574 MB)", nota: "Pode acertar mais em português, mas só com processador leva 20–35 s por ditado curto. Usa ~1,2 GB de memória enquanto ativo." },
];
const OCIOSO_OPCOES: { v: VoiceConfig["whisperOciosoMin"]; l: string }[] = [
  { v: 0, l: "Logo após cada ditado" },
  { v: 2, l: "2 minutos" },
  { v: 5, l: "5 minutos" },
  { v: 15, l: "15 minutos" },
];

/** Ditado local (Whisper embutido): baixar/remover cada modelo, escolher o que usar e em quanto tempo soltar a memória.
 *  A escolha do modelo e do prazo vai para o `cfg` da página e grava no botão Salvar, como o resto da Voz. */
function WhisperLocal({ cfg, set, onMudou }: { cfg: VoiceConfig; set: (p: Partial<VoiceConfig>) => void; onMudou: () => void }) {
  const [st, setSt] = useState<WhisperLocalStatus | null>(null);
  const [prog, setProg] = useState<{ nome: string; bytes: number; total: number } | null>(null);
  const [alvo, setAlvo] = useState<WhisperModelo | null>(null);
  const [busy, setBusy] = useState(false);

  const recarregar = () => api.whisperLocalStatus().then(setSt).catch(toast);
  useEffect(() => {
    recarregar();
  }, []);

  if (!st) return null;

  const instalados = MODELOS_WHISPER.filter((m) => st.modelos[m.id].instalado).map((m) => m.id);
  // o escolhido se estiver baixado; senão o outro (o backend faz o mesmo)
  const usado = instalados.includes(cfg.whisperModelo) ? cfg.whisperModelo : (instalados[0] ?? null);
  const ocupado = busy || st.instalando;

  async function instalar(id: WhisperModelo) {
    setBusy(true);
    setAlvo(id);
    try {
      await api.whisperLocalInstalar(id, (e) => {
        if (e.evento === "arquivo") setProg({ nome: e.nome, bytes: e.bytes, total: e.total });
      });
      useStore.getState().toast("Modelo baixado: o ditado agora roda no computador", "ok");
      onMudou();
    } catch (e) {
      if (errCode(e) === "cancelado") useStore.getState().toast("Download cancelado", "info");
      else toast(e);
    } finally {
      setBusy(false);
      setAlvo(null);
      setProg(null);
      recarregar();
    }
  }

  async function remover(id: WhisperModelo) {
    const ultimo = instalados.length === 1;
    const aviso = ultimo
      ? "Remover o último modelo? O motor do Whisper local também sai e o ditado volta para a nuvem (Groq). Você pode baixar de novo depois."
      : "Remover este modelo? Você pode baixar de novo depois.";
    if (!window.confirm(aviso)) return;
    setBusy(true);
    try {
      await api.whisperLocalRemover(id);
      useStore.getState().toast("Modelo removido", "ok");
      onMudou();
    } catch (e) {
      toast(e);
    } finally {
      setBusy(false);
      recarregar();
    }
  }

  return (
    <>
      <h4>Ditado local (Whisper)</h4>
      <p className="meta">Transcrição que roda no seu computador, sem chave e sem custo. Baixe um ou os dois modelos (whisper.cpp, licença MIT).</p>
      {!st.binarios && <p className="meta">O primeiro download inclui o motor do whisper.cpp (~{Math.round(st.bytesZip / 1e6)} MB).</p>}
      {MODELOS_WHISPER.map((m) => {
        const ok = st.modelos[m.id].instalado;
        const baixando = busy && alvo === m.id;
        return (
          <div key={m.id} className="field">
            <label className="row">
              <input type="radio" name="whisper-modelo" disabled={!ok} checked={usado === m.id} onChange={() => set({ whisperModelo: m.id })} />
              <strong>{m.titulo}</strong>
            </label>
            <p className="meta">{m.nota}</p>
            <div className="row">
              {ok ? (
                <button className="btn" disabled={ocupado} onClick={() => remover(m.id)}>Remover</button>
              ) : (
                <button className="btn primary" disabled={ocupado} onClick={() => instalar(m.id)}>Baixar</button>
              )}
              {baixando && <button className="btn" onClick={() => api.whisperLocalCancelar().catch(toast)}>Cancelar</button>}
            </div>
            {baixando && prog && (
              <div className="row">
                <span className="meta">{prog.nome} · {Math.round((prog.bytes / Math.max(prog.total, 1)) * 100)}%</span>
                <span className="lvl-bar" role="meter" aria-label="Progresso do arquivo" aria-valuenow={Math.round((prog.bytes / Math.max(prog.total, 1)) * 100)}>
                  <i style={{ width: `${(prog.bytes / Math.max(prog.total, 1)) * 100}%` }} />
                </span>
              </div>
            )}
          </div>
        );
      })}
      {!busy && st.instalando && <p className="meta">Download em andamento…</p>}
      {instalados.length > 0 && (
        <>
          <label className="field"><span>Liberar a memória do ditado</span>
            <select value={cfg.whisperOciosoMin} onChange={(e) => set({ whisperOciosoMin: Number(e.target.value) as VoiceConfig["whisperOciosoMin"] })}>
              {OCIOSO_OPCOES.map((o) => <option key={o.v} value={o.v}>{o.l}</option>)}
            </select>
          </label>
          <p className="meta">Entre um ditado e outro o modelo fica na memória, o que deixa o próximo mais rápido. Escolher "logo após" devolve a memória na hora, mas cada ditado recarrega o modelo e demora mais. Clique em Salvar no fim da página para aplicar a escolha do modelo e do tempo.</p>
        </>
      )}
    </>
  );
}

export default function VoiceSection() {
  const [cfg, setCfg] = useState<VoiceConfig | null>(null);
  const [terms, setTerms] = useState("");
  const [priceErr, setPriceErr] = useState("");
  const [vault, setVault] = useState<string[]>([]);
  const [secretValue, setSecretValue] = useState("");
  const [capturing, setCapturing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [meter, setMeter] = useState(0);
  const [capturingGlobal, setCapturingGlobal] = useState(false);
  const [history, setHistory] = useState<VoiceEntry[]>([]);

  useEffect(() => {
    api.voiceGet().then((c) => { setCfg(c); setTerms(c.terms.join(", ")); }).catch(toast);
    api.vaultNames().then(setVault).catch(toast);
    api.voiceHistoryList().then(setHistory).catch(toast);
  }, []);

  async function clearHistory() {
    try {
      await api.voiceHistoryClear();
      setHistory([]);
    } catch (e) {
      toast(e);
    }
  }

  if (!cfg) return <><h3>Voz</h3><div className="meta">Carregando…</div></>;

  const set = (p: Partial<VoiceConfig>) => setCfg({ ...cfg, ...p });
  const local = cfg.engine === "local";
  const keyName = cfg.secret ?? "voice.groq";

  async function save() {
    if (!cfg || busy) return;
    setBusy(true);
    try {
      await api.voiceSave({ ...cfg, terms: parseTerms(terms) });
      const fresh = await api.voiceGet();
      setCfg(fresh);
      setTerms(fresh.terms.join(", "));
      useStore.getState().patch({ voice: fresh });
      useStore.getState().toast("Voz salva", "ok");
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

  /** Lê 3 s do microfone para mostrar o nível, sem gravar nada. */
  async function testMic() {
    setTesting(true);
    const r = new VoiceRecorder();
    try {
      await r.start();
      await new Promise<void>((resolve) => {
        const t0 = Date.now();
        const id = window.setInterval(() => {
          setMeter(r.level());
          if (Date.now() - t0 >= 3000) {
            clearInterval(id);
            resolve();
          }
        }, 80);
      });
    } catch (e) {
      useStore.getState().toast(micMessage(e), "erro");
    } finally {
      r.cancel();
      setMeter(0);
      setTesting(false);
    }
  }

  return (
    <>
      <h3>Voz</h3>
      <p style={{ color: "var(--fg3)" }}>Ditado no painel ativo. O áudio só vai ao motor escolhido abaixo.</p>

      <VozLocal />
      <WhisperLocal cfg={cfg} set={set} onMudou={() => api.voiceGet().then((v) => { setCfg(v); useStore.getState().patch({ voice: v }); }).catch(toast)} />

      <h4>Motor</h4>
      <label className="field"><span>Motor de transcrição</span>
        <select value={cfg.engine} onChange={(e) => set({ engine: e.target.value as VoiceConfig["engine"] })}>
          {Object.entries(ENGINES).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
        </select>
      </label>
      {local ? (
        <details>
          <summary>Avançado: usar um whisper.cpp próprio</summary>
          <label className="field"><span>Caminho do whisper-server.exe</span>
            <input value={cfg.whisperServer ?? ""} placeholder="C:\\whisper\\whisper-server.exe" onChange={(e) => set({ whisperServer: e.target.value || null })} />
          </label>
          <label className="field"><span>Modelo (.bin)</span>
            <input value={cfg.whisperModel ?? ""} placeholder="C:\\whisper\\ggml-large-v3-turbo.bin" onChange={(e) => set({ whisperModel: e.target.value || null })} />
          </label>
          <p className="meta">Com o Whisper local baixado acima, não precisa preencher nada. Estes caminhos substituem o embutido.</p>
        </details>
      ) : (
        <>
          <label className="field"><span>URL base</span>
            <input value={cfg.baseUrl} placeholder="https://api.groq.com/openai/v1" onChange={(e) => set({ baseUrl: e.target.value })} />
          </label>
          <div className="row">
            <label className="field"><span>Modelo (conversa)</span><input value={cfg.modelChat} onChange={(e) => set({ modelChat: e.target.value })} /></label>
            <label className="field"><span>Modelo (código)</span><input value={cfg.modelCode} onChange={(e) => set({ modelCode: e.target.value })} /></label>
          </div>
          <label className="field"><span>Nome da chave no Cofre</span>
            <input value={cfg.secret ?? ""} onChange={(e) => set({ secret: e.target.value || null })} />
          </label>
          <div className="row">
            <input type="password" placeholder="valor da chave" aria-label="Chave da API de voz" value={secretValue} onChange={(e) => setSecretValue(e.target.value)} />
            <button className="btn" disabled={!secretValue} onClick={saveKey}>Salvar chave</button>
            <span className="meta">{vault.includes(keyName) ? "chave salva no Cofre" : "sem chave no Cofre"}</span>
          </div>
        </>
      )}

      <h4>Opções</h4>
      <div className="row">
        <label className="field"><span>Idioma</span>
          <select value={cfg.language} onChange={(e) => set({ language: e.target.value as VoiceConfig["language"] })}>
            {Object.entries(LANGS).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
        </label>
        <label className="field"><span>Modo (vocabulário)</span>
          <select value={cfg.mode} onChange={(e) => set({ mode: e.target.value as VoiceConfig["mode"] })}>
            {Object.entries(MODES).map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
        </label>
      </div>
      <label className="row"><input type="checkbox" checked={cfg.toEnglish} onChange={(e) => set({ toEnglish: e.target.checked })} /> Saída em inglês</label>
      <label className="row"><input type="checkbox" checked={cfg.autoEnter} onChange={(e) => set({ autoEnter: e.target.checked })} /> Enviar com Enter automaticamente</label>
      <label className="field"><span>Preço por minuto de áudio (US$, opcional; soma o gasto de voz no Uso)</span>
        <input inputMode="decimal" defaultValue={cfg.usdPerMin || ""} aria-invalid={priceErr !== ""} onBlur={(e) => {
          const raw = e.target.value.trim();
          const n = Number(raw.replace(",", "."));
          if (raw === "" || (Number.isFinite(n) && n >= 0)) {
            setPriceErr("");
            set({ usdPerMin: n || 0 });
          } else {
            // valor inválido: avisa e volta ao preço salvo
            setPriceErr("Preço inválido: use um número igual ou maior que zero.");
            e.target.value = String(cfg.usdPerMin || "");
          }
        }} />
        {priceErr && <span role="alert" className="meta">{priceErr}</span>}
      </label>
      <label className="field"><span>Termos do projeto (separados por vírgula)</span>
        <textarea rows={2} value={terms} onChange={(e) => setTerms(e.target.value)} />
      </label>

      <h4>Tecla</h4>
      <div className="row">
        <input
          readOnly
          aria-label="Tecla de ditado (clique e pressione a combinação)"
          value={capturing ? "pressione a combinação…" : cfg.hotkey}
          onFocus={() => setCapturing(true)}
          onBlur={() => setCapturing(false)}
          onKeyDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            const k = formatHotkey(e);
            if (k) {
              set({ hotkey: k });
              e.currentTarget.blur();
            }
          }}
        />
        <label className="row"><input type="radio" name="voz-tecla" checked={cfg.hold} onChange={() => set({ hold: true })} /> Segurar</label>
        <label className="row"><input type="radio" name="voz-tecla" checked={!cfg.hold} onChange={() => set({ hold: false })} /> Alternar</label>
      </div>
      <p className="meta">Segurar grava enquanto a tecla está apertada. Alternar: aperta para começar e de novo para enviar. Esc cancela.</p>

      <h4>Ditado em qualquer app</h4>
      <label className="row"><input type="checkbox" checked={cfg.globalEnabled} onChange={(e) => set({ globalEnabled: e.target.checked })} /> Ativar o ditado global (atalho do sistema, cola no app focado)</label>
      <div className="row">
        <input
          readOnly
          aria-label="Atalho do ditado global (clique e pressione a combinação)"
          value={capturingGlobal ? "pressione a combinação…" : cfg.globalHotkey}
          onFocus={() => setCapturingGlobal(true)}
          onBlur={() => setCapturingGlobal(false)}
          onKeyDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            const k = formatHotkey(e);
            if (k) {
              set({ globalHotkey: k });
              e.currentTarget.blur();
            }
          }}
        />
        <label className="row"><input type="radio" name="voz-global-tecla" checked={cfg.globalHold} onChange={() => set({ globalHold: true })} /> Segurar</label>
        <label className="row"><input type="radio" name="voz-global-tecla" checked={!cfg.globalHold} onChange={() => set({ globalHold: false })} /> Alternar</label>
        <label className="field"><span>Modo</span>
          <select value={cfg.globalMode} onChange={(e) => set({ globalMode: e.target.value as VoiceConfig["globalMode"] })}>
            <option value="chat">Chat (PT, fiel)</option>
            <option value="code">Código (PT → EN)</option>
          </select>
        </label>
      </div>
      {/Alt/.test(cfg.globalHotkey) && (
        <p className="meta" role="alert" style={{ color: "var(--warn, #d9a400)" }}>
          Atalhos com Alt podem falhar ao colar em alguns apps (ex.: Bloco de Notas).
        </p>
      )}
      <p className="meta">O texto transcrito é enviado ao provedor da IA auxiliar (Anthropic/OpenAI) para a tradução.</p>
      <p className="meta">
        Modo código: com a IA auxiliar configurada, o texto vira inglês mantendo os literais; sem ele, usa a tradução do Whisper.
        Se o Regente estiver em foco, o texto vai só para a área de transferência. Se a colagem falhar, o aviso mostra onde está o texto.
      </p>

      <h4>Histórico do ditado global</h4>
      {history.length === 0 ? (
        <p className="meta">Nenhum ditado global ainda.</p>
      ) : (
        <ul className="voice-history">
          {history.map((h) => (
            <li key={h.id}><span className="meta">{new Date(h.ts).toLocaleString("pt-BR")} · {h.mode === "code" ? "código" : "chat"}</span> {h.text}</li>
          ))}
        </ul>
      )}
      <div className="row">
        <button className="btn" disabled={history.length === 0} onClick={clearHistory}>Limpar</button>
      </div>

      <div className="row" style={{ marginTop: 10 }}>
        <button className="btn" disabled={testing} onClick={testMic}>{testing ? "Ouvindo…" : "Testar microfone"}</button>
        {testing && (
          <span className="lvl-bar" role="meter" aria-label="Nível do microfone" aria-valuenow={Math.round(meter * 100)}>
            <i style={{ width: `${meter * 100}%` }} />
          </span>
        )}
        <span className="spacer" />
        <button className="btn primary" disabled={busy} onClick={save}>Salvar</button>
      </div>
    </>
  );
}
