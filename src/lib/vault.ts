/** Escopo de um segredo: `null` = global (todos os workspaces); senão o id do workspace. Nome salvo: `ws:<id>:<nome>`. */
export function scopedVaultName(workspaceId: string | null, name: string): string {
  return workspaceId ? `ws:${workspaceId}:${name.trim()}` : name.trim();
}

/** Pura: separa o nome salvo no escopo (workspace id ou null) e no nome que o usuário vê. */
export function parseVaultName(saved: string): { scope: string | null; name: string } {
  const m = /^ws:([^:]+):(.+)$/.exec(saved);
  return m ? { scope: m[1], name: m[2] } : { scope: null, name: saved };
}
