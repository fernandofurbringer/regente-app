import { useState } from "react";
import { api, errMsg } from "../lib/ipc";

/** Barra "Parar" da gravação (Shot): pequena, sempre no topo; enquanto ela estiver aberta, o ffmpeg grava. */
export default function ShotRecordBar() {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  async function stop() {
    setBusy(true);
    try {
      // the backend closes this bar when the file is finished
      await api.shotRecordStop();
    } catch (e) {
      setErr(errMsg(e));
      setBusy(false);
    }
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, height: "100vh", padding: "0 10px", background: "#1b1b1b", color: "#fff", font: "13px sans-serif", userSelect: "none" }}>
      <span aria-hidden style={{ width: 10, height: 10, borderRadius: 5, background: "#e53935" }} />
      <span>REC</span>
      <button onClick={stop} disabled={busy} style={{ marginLeft: "auto", padding: "4px 10px" }}>Parar</button>
      {err && <span role="alert" title={err} style={{ color: "#ffb4a9" }}>!</span>}
    </div>
  );
}
