import type { Perfil, PerfilSlot } from "./ipc";
import type { RPayload } from "./redline";

/** Uma linha da tabela "Quem faz o quê": onde o valor fica no perfil, o nome e o que o papel faz. */
export interface LinhaPapel { chave: string; grupo: "papeis" | "modelos"; campo: string; nome: string; dica: string }

export const LINHAS: LinhaPapel[] = [
  { chave: "piloto", grupo: "papeis", campo: "piloto", nome: "Piloto", dica: "orquestra: quebra o pedido e aciona os chefes" },
  { chave: "scout", grupo: "papeis", campo: "scout", nome: "Chefe scout", dica: "comanda a pesquisa" },
  { chave: "builder", grupo: "papeis", campo: "builder", nome: "Chefe builder", dica: "comanda a construção" },
  { chave: "reviewer", grupo: "papeis", campo: "reviewer", nome: "Chefe reviewer", dica: "comanda a revisão e dá o veredito" },
  { chave: "controller", grupo: "papeis", campo: "controller", nome: "Chefe controller", dica: "acompanha prazo e custo (estilo torre)" },
  { chave: "cruzado", grupo: "papeis", campo: "cruzado", nome: "Revisor cruzado", dica: "outra IA confere os achados do reviewer (opcional)" },
  { chave: "implementar", grupo: "modelos", campo: "implementar", nome: "Executores", dica: "quem escreve o código e os artefatos" },
  { chave: "revisar", grupo: "modelos", campo: "revisar", nome: "Revisores", dica: "quem revisa um eixo (código, segurança…)" },
  { chave: "explorar", grupo: "modelos", campo: "explorar", nome: "Olheiros", dica: "quem pesquisa (web, docs, código)" },
  { chave: "testar", grupo: "modelos", campo: "testar", nome: "Testadores", dica: "quem escreve e roda testes" },
  { chave: "planejar", grupo: "modelos", campo: "planejar", nome: "Planejador", dica: "o plano do estilo Plano → Execução" },
];

/** Os slots que a tabela edita, de um perfil. */
export type Tabela = Record<string, PerfilSlot | null>;

export function tabelaDe(p: Perfil): Tabela {
  const t: Tabela = {};
  for (const l of LINHAS) t[l.chave] = (l.grupo === "papeis" ? (p.papeis as unknown as Record<string, PerfilSlot | null>)[l.campo] : (p.modelos as unknown as Record<string, PerfilSlot>)[l.campo]) ?? null;
  return t;
}

/** A tabela de volta para os campos do perfil (`papeis` e `modelos`), com a CLI sempre explícita. */
export function camposDe(t: Tabela, base: Perfil): Pick<Perfil, "papeis" | "modelos"> {
  const papeis = { ...base.papeis } as unknown as Record<string, PerfilSlot | null>;
  const modelos = { ...base.modelos } as unknown as Record<string, PerfilSlot>;
  for (const l of LINHAS) {
    const v = t[l.chave];
    const slot = v ? { cli: v.cli ?? "claude", model: v.model, effort: v.effort } : null;
    if (l.grupo === "papeis") papeis[l.campo] = slot;
    else if (slot) modelos[l.campo] = slot;
  }
  return { papeis: papeis as unknown as Perfil["papeis"], modelos: modelos as unknown as Perfil["modelos"] };
}

/** Família do modelo para achar a janela de limite própria dela ("Sonnet only", Spark). */
export function familia(model: string): string | null {
  return model.match(/opus|sonnet|haiku|spark/i)?.[0].toLowerCase() ?? null;
}

/**
 * O limite que o papel consome, pelo Redline: a janela própria da família do modelo (ex.: `seven_day_sonnet`), senão
 * a janela geral mais cheia da CLI. null sem leitura.
 */
export function limiteDoPapel(limits: RPayload | null, slot: PerfilSlot, conta = "principal"): { janela: string; pct: number } | null {
  if (!limits) return null;
  const cli = slot.cli ?? "claude";
  const ws = limits.windows.filter((w) => w.provider === cli && w.account === conta);
  if (!ws.length) return null;
  const f = familia(slot.model);
  const propria = f ? ws.find((w) => w.window.toLowerCase().includes(f)) : undefined;
  if (propria) return { janela: propria.window, pct: propria.usedPercent };
  const geral = ws.filter((w) => !/opus|sonnet|haiku|spark/i.test(w.window)).sort((a, b) => b.usedPercent - a.usedPercent)[0];
  return geral ? { janela: geral.window, pct: geral.usedPercent } : null;
}
