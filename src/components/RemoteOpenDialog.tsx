import { useEffect, useState, type FormEvent } from "react";
import { api } from "../lib/ipc";
import { remoteTarget } from "../lib/remote";
import { useStore } from "../state/store";
import Modal from "./Modal";

/** Abre um workspace numa pasta de um servidor SSH. Os pré-requisitos (ssh sem senha, tmux) são checados pelo backend
 *  e a mensagem de cada um aparece como aviso. */
export default function RemoteOpenDialog({ onClose }: { onClose(): void }) {
  const openWorkspace = useStore((s) => s.openWorkspace);
  const [aliases, setAliases] = useState<string[]>([]);
  const [alias, setAlias] = useState("");
  const [path, setPath] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.sshAliases().then(setAliases).catch(() => setAliases([]));
  }, []);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const t = remoteTarget(alias, path);
    if ("error" in t) return setErr(t.error);
    setErr(null);
    setBusy(true);
    // erro do backend (ssh, tmux) vira aviso pelo próprio openWorkspace; o diálogo fica aberto
    const ws = await openWorkspace(t.url);
    setBusy(false);
    if (ws) onClose();
  }

  return (
    <Modal label="Abrir workspace remoto" onClose={onClose}>
      <form onSubmit={submit} style={{ display: "grid", gap: 4, minWidth: 360 }}>
        <h2>Abrir remoto</h2>
        <div className="field">
          <label htmlFor="remote-alias">Servidor (alias do ~/.ssh/config)</label>
          <input id="remote-alias" list="remote-aliases" value={alias} onChange={(e) => setAlias(e.target.value)} placeholder="ex.: meu-servidor" autoComplete="off" />
          <datalist id="remote-aliases">
            {aliases.map((a) => <option key={a} value={a} />)}
          </datalist>
        </div>
        <div className="field">
          <label htmlFor="remote-path">Pasta absoluta no servidor</label>
          <input id="remote-path" value={path} onChange={(e) => setPath(e.target.value)} placeholder="/home/usuario/projeto" autoComplete="off" />
        </div>
        <small style={{ color: "var(--fg3)" }}>Use chave SSH no ssh-agent. O Regente não guarda senhas. O servidor precisa ter tmux.</small>
        {err && <div role="alert" style={{ color: "var(--danger, #e5484d)" }}>{err}</div>}
        <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
          <button type="button" className="btn" onClick={onClose}>Cancelar</button>
          <button type="submit" className="btn primary" disabled={busy}>{busy ? "Conectando…" : "Abrir"}</button>
        </div>
      </form>
    </Modal>
  );
}
