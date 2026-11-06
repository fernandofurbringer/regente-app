import { describe, expect, it } from "vitest";
import type { IntakeQ, RecipeDef, SquadDef } from "./ipc";
import { applyRecipe, autofill, briefFromIssue, chosenSlots, initialWizard, memoryOf, missingFor, missionInput, pickSquad, presetBuilderReviewer, presetCompare, promptOf, setBrief, withMemory, type WizardState } from "./wizard";

const INTAKE: IntakeQ[] = [
  { q: "URL do site", autofill: "first_url" },
  { q: "Referência", autofill: "inspired_by" },
  { q: "Cor principal", autofill: null },
];
const SQUAD = { id: "jogo-web", name: "Jogo Web", description: "", intake: INTAKE, roster: [], phases: [], maestroNotes: "", maestro: null, peakPanes: 4, source: "original" } as SquadDef;
const base = (): WizardState => initialWizard({ kind: "agent", cli: "claude", permission: "pend" }, "chain");

describe("autofill (mesmas regras de squads::autofill)", () => {
  it("primeira URL e referência após 'inspirado em'", () => {
    expect(autofill(INTAKE, "site pra minha loja https://loja.com inspirado no apple.com")).toEqual({ 0: "https://loja.com", 1: "apple.com" });
  });
  it("sem pistas = vazio", () => {
    expect(autofill(INTAKE, "nada aqui")).toEqual({});
  });
  it("pontos finais saem da URL", () => {
    expect(autofill(INTAKE, "veja https://a.com/x.")[0]).toBe("https://a.com/x");
  });
  it("só preenche perguntas com a regra", () => {
    expect(autofill(INTAKE, "https://a.com")).not.toHaveProperty("2");
  });
});

describe("setBrief", () => {
  it("preenche o que o brief dá e marca como automático", () => {
    const s = setBrief(base(), INTAKE, "https://loja.com");
    expect(s.answers[0]).toBe("https://loja.com");
    expect(s.answers[1]).toBeUndefined();
  });
  it("não sobrescreve resposta editada pelo usuário", () => {
    let s = setBrief(base(), INTAKE, "https://loja.com");
    s = { ...s, answers: { ...s.answers, 0: "https://minha.com" } };
    s = setBrief(s, INTAKE, "https://outra.com");
    expect(s.answers[0]).toBe("https://minha.com");
  });
  it("atualiza o preenchimento automático e remove quando o brief deixa de dar a pista", () => {
    let s = setBrief(base(), INTAKE, "https://loja.com");
    s = setBrief(s, INTAKE, "https://outra.com");
    expect(s.answers[0]).toBe("https://outra.com");
    s = setBrief(s, INTAKE, "sem link");
    expect(s.answers[0]).toBeUndefined();
  });
});

describe("applyRecipe", () => {
  const recipe = (over: Partial<RecipeDef>): RecipeDef => ({
    id: "r1", name: "R", description: "", mode: "squad", squad: "jogo-web", pilot: null, slots: [],
    intake: { "Referência": "snake.io" }, worktree: true, style: null, source: "regente", ...over,
  });
  it("modo, squad, worktree e respostas por pergunta", () => {
    const s = applyRecipe(recipe({}), { ...base(), brief: "https://loja.com" }, SQUAD);
    expect(s.mode).toBe("squad");
    expect(s.recipeId).toBe("r1");
    expect(s.squadId).toBe("jogo-web");
    expect(s.worktree).toBe(true);
    expect(s.answers).toEqual({ 0: "https://loja.com", 1: "snake.io" });
  });
  it("piloto e estilo do agêntico", () => {
    const s = applyRecipe(recipe({ mode: "agentico", squad: null, pilot: { cli: "claude", model: "haiku-5-5", effort: null }, style: "torre" }), base());
    expect(s.mode).toBe("agentico");
    expect(s.pilot).toEqual({ cli: "claude", model: "haiku-5-5", effort: null, account: null });
    expect(s.style).toBe("torre");
  });
  it("modo livre usa os terminais da receita", () => {
    const s = applyRecipe(recipe({ mode: "livre", squad: null, worktree: null, slots: [{ kind: "terminal" }, { kind: "agent", cli: "codex" }] }), base());
    expect(s.count).toBe(2);
    expect(s.individual).toBe(true);
    expect(chosenSlots(s)[1]).toEqual({ kind: "agent", cli: "codex" });
    expect(s.worktree).toBe(false);
  });
});

describe("missionInput e validação", () => {
  const ctx = { workspaceId: "w", cwd: "C:\p" };
  it("squad envia brief, squadId e só respostas preenchidas", () => {
    const s = { ...setBrief({ ...base(), mode: "squad", name: " Jogo " }, INTAKE, "https://loja.com"), squadId: "jogo-web", answers: { 0: "https://loja.com", 2: "  " } };
    expect(missionInput(s, ctx)).toEqual({ workspaceId: "w", name: "Jogo", worktree: false, mode: "squad", squadId: "jogo-web", brief: "https://loja.com", answers: { 0: "https://loja.com" } });
  });
  it("agêntico envia piloto e estilo", () => {
    const s = { ...base(), mode: "agentico" as const, brief: "delegue um README", pilot: { cli: "claude", model: "haiku-5-5", effort: null, account: null }, style: "torre" as const };
    const m = missionInput(s, ctx);
    expect(m.mode).toBe("agentico");
    expect(m.pilot).toEqual(s.pilot);
    expect(m.style).toBe("torre");
  });
  it("livre envia os terminais com cwd", () => {
    const m = missionInput({ ...base(), count: 1 }, ctx);
    expect(m.slots).toEqual([{ kind: "agent", cli: "claude", permission: "pend", workspaceId: "w", cwd: "C:\p" }]);
  });
  it("squad sem squad ou brief não inicia", () => {
    expect(missingFor({ ...base(), mode: "squad" })).toBe("escolha uma squad");
    expect(missingFor({ ...base(), mode: "squad", squadId: "jogo-web" })).toBe("escreva o pedido da missão");
    expect(missingFor({ ...base(), mode: "agentico", brief: "x" })).toBeNull();
    expect(missingFor(base())).toBeNull();
  });
  it("pickSquad troca as respostas pelas do intake da squad", () => {
    const s = pickSquad({ ...base(), brief: "https://a.com" }, SQUAD);
    expect(s.squadId).toBe("jogo-web");
    expect(s.answers).toEqual({ 0: "https://a.com" });
  });
});

describe("pedido inicial do modo livre (B-20)", () => {
  const livre = (over: Partial<WizardState>): WizardState => ({ ...base(), count: 3, prompt: "  corrija o login  ", ...over });
  const ctx = { workspaceId: "w", cwd: "C:/p" };
  it("vai a todos os terminais de IA, sem espaços nas pontas", () => {
    const slots = chosenSlots(livre({}));
    expect(slots.map((s) => s.prompt)).toEqual(["corrija o login", "corrija o login", "corrija o login"]);
    expect(missionInput(livre({ count: 1 }), ctx).slots?.[0].prompt).toBe("corrija o login");
  });
  it("desmarcado, nenhum terminal recebe o pedido compartilhado", () => {
    expect(chosenSlots(livre({ sendToAll: false })).every((s) => s.prompt === undefined)).toBe(true);
  });
  it("editado num terminal, vale só para ele", () => {
    const st = livre({ individual: true, sendToAll: false, slots: [{ kind: "agent", cli: "claude", prompt: "revise o CSS" }] });
    expect(chosenSlots(st).map((s) => s.prompt)).toEqual(["revise o CSS", undefined, undefined]);
    expect(promptOf(livre({ individual: true, slots: [{ kind: "agent", cli: "claude", prompt: "" }] }), 0)).toBeNull();
  });
  it("terminal que não é de IA não recebe pedido", () => {
    expect(chosenSlots(livre({ count: 1, template: { kind: "terminal", cli: null, shell: "powershell" } }))[0]).not.toHaveProperty("prompt");
  });
});

describe("briefFromIssue", () => {
  it("monta nome e brief com rótulos", () => {
    const r = briefFromIssue({ number: 12, title: "  Botão quebrado ", body: "  Passos:\n1. clicar  ", labels: ["bug", "ui"] });
    expect(r.name).toBe("#12 Botão quebrado");
    expect(r.brief).toBe("Issue #12: Botão quebrado\nRótulos: bug, ui\n\nPassos:\n1. clicar");
  });

  it("sem rótulos e sem corpo, o brief é só o título", () => {
    expect(briefFromIssue({ number: 3, title: "Ajuste", body: "", labels: [] }).brief).toBe("Issue #3: Ajuste");
  });

  it("o nome corta em 80 caracteres sem partir letras acentuadas", () => {
    const r = briefFromIssue({ number: 1, title: "ç".repeat(200), body: "", labels: [] });
    expect(Array.from(r.name)).toHaveLength(80);
  });
});

describe("memória e predefinições do assistente", () => {
  it("a memória da missão volta no assistente novo, sem o pedido do terminal", () => {
    const s = { ...base(), mode: "squad" as const, count: 5, worktree: true, template: { kind: "agent" as const, cli: "codex", permission: "auto" as const, prompt: "pedido antigo" } };
    const mem = JSON.parse(JSON.stringify(memoryOf(s)));
    const fresh = withMemory(base(), mem);
    expect([fresh.mode, fresh.count, fresh.worktree, fresh.template.cli, fresh.template.permission]).toEqual(["squad", 5, true, "codex", "auto"]);
    expect(fresh.template.prompt).toBeNull();
    expect(withMemory(base(), null)).toEqual(base());
  });

  it("'Comparar 3 modelos' deixa 3 terminais configuráveis; 'Builder + reviewer' põe os papéis", () => {
    const cmp = presetCompare(base());
    expect([cmp.mode, cmp.count, cmp.individual, cmp.slots.length]).toEqual(["livre", 3, true, 3]);
    const br = presetBuilderReviewer(base());
    expect([br.count, br.slots.map((x) => x.role)]).toEqual([2, ["builder", "reviewer"]]);
  });
});
