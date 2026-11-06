import { wsUrl } from "./api";

/** Avisos do celular (PWA aberto). O servidor manda só o tipo e o id do painel; o texto aqui é fixo, sem prompt. */
export interface Notice { kind: "finished" | "asks" | "gate"; paneId: string }

export function noticeText(kind: string): { title: string; body: string } | null {
  switch (kind) {
    case "finished":
      return { title: "Agente terminou", body: "Um painel voltou a ficar ocioso." };
    case "asks":
      return { title: "Agente perguntou", body: "Um painel está esperando sua resposta." };
    case "gate":
      return { title: "Gate aguardando decisão", body: "Abra Pendências para responder." };
    default:
      return null;
  }
}

/** Lê um quadro do servidor; quadro que não é aviso conhecido vira null. */
export function parseNotice(raw: string): Notice | null {
  try {
    const v = JSON.parse(raw) as { kind?: unknown; paneId?: unknown };
    if (typeof v.paneId !== "string" || v.paneId.length === 0 || noticeText(String(v.kind)) === null) return null;
    return { kind: v.kind as Notice["kind"], paneId: v.paneId };
  } catch {
    return null;
  }
}

/** Mostra o aviso se a permissão foi dada. Uma notificação por painel (a nova substitui a antiga). */
export function showNotice(n: Notice): void {
  const text = noticeText(n.kind);
  if (!text || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const note = new Notification(text.title, { body: text.body, tag: n.paneId });
  note.onclick = () => {
    window.focus();
    note.close();
  };
}

/** Conexão de avisos: envia o token como primeiro quadro e reconecta enquanto a página estiver aberta. */
export function openNotices(token: string, onNotice: (n: Notice) => void): () => void {
  let ws: WebSocket | null = null;
  let timer: number | undefined;
  let closed = false;
  const connect = () => {
    ws = new WebSocket(wsUrl("/ws/notices", window.location));
    ws.onopen = () => ws?.send(JSON.stringify({ type: "auth", token }));
    ws.onmessage = (e) => {
      const n = typeof e.data === "string" ? parseNotice(e.data) : null;
      if (n) onNotice(n);
    };
    ws.onclose = (e) => {
      // 1008 = o servidor recusou (token, revogação, limite): não insiste
      if (!closed && e.code !== 1008) timer = window.setTimeout(connect, 5000);
    };
  };
  connect();
  return () => {
    closed = true;
    window.clearTimeout(timer);
    ws?.close();
  };
}
