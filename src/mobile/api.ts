// Cliente da PWA do celular: só fetch e WebSocket no mesmo servidor. Nada de Tauri aqui.

const TOKEN_KEY = "regente.celular.token";

/** Token do aparelho (guardado só neste navegador). Leitura e escrita protegidas: o modo privado pode recusar. */
export function storedToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function saveToken(token: string | null): void {
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // sem armazenamento: o aparelho pareia de novo ao recarregar
  }
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** Chamada à API do servidor. Erros viram ApiError com o código estável do servidor. */
export async function call<T>(method: "GET" | "POST", path: string, body?: unknown, token?: string | null): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  if (token) headers.authorization = `Bearer ${token}`;
  const r = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    cache: "no-store",
    credentials: "omit",
  });
  const data = await r.json().catch(() => ({}));
  if (!r.ok) throw new ApiError(r.status, String(data.error ?? "erro"), String(data.message ?? `HTTP ${r.status}`));
  return data as T;
}

/** Estado de aprovação de um celular pareado por código. */
export type ApprovalState = "approved" | "waiting" | "refused" | "error";

/** Transição a partir da resposta de uma chamada autenticada: 200 aprovado; 403 pending_approval aguardando; 401 recusado. */
export function approvalState(status: number | null, code?: string): ApprovalState {
  if (status === 200) return "approved";
  if (status === 403 && code === "pending_approval") return "waiting";
  if (status === 401) return "refused";
  return "error";
}

/** Consulta a aprovação com uma chamada leve (lista de workspaces). Sem conexão ou resposta inesperada: "error". */
export async function checkApproval(token: string): Promise<ApprovalState> {
  try {
    await call("GET", "/api/workspaces", undefined, token);
    return "approved";
  } catch (e) {
    if (e instanceof ApiError) return approvalState(e.status, e.code);
    return "error";
  }
}

/** Token de uso único do link `#pair=<token>` (64 hex), ou null. */
export function pairTokenFromHash(hash: string): string | null {
  const m = /^#pair=([0-9a-f]{64})$/.exec(hash);
  return m ? m[1] : null;
}

/** Código de pareamento de 6 dígitos. */
export function isPairCode(text: string): boolean {
  return /^\d{6}$/.test(text.trim());
}

/** URL do WebSocket no mesmo host da página (ws: ou wss: conforme a página). */
export function wsUrl(path: string, loc: Pick<Location, "protocol" | "host">): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}${path}`;
}
