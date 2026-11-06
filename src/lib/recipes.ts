/** Perguntas e respostas do intake da receita, na ordem de edição. Pergunta vazia some; resposta vazia fica. */
export interface IntakePair { q: string; a: string }

export const intakeToPairs = (intake: Record<string, string>): IntakePair[] => Object.entries(intake).map(([q, a]) => ({ q, a }));

export function pairsToIntake(pairs: IntakePair[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of pairs) {
    const q = p.q.trim();
    if (q) out[q] = p.a;
  }
  return out;
}
