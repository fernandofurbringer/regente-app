/** Tema do Monaco para o tema do app. "regente-aurora" = TEMA_ESCURO de monaco.ts (importar de lá traria o Monaco
 *  inteiro para quem só quer o nome); "hc-black" é o alto contraste que vem no Monaco. */
export function editorTheme(t: string | undefined): string {
  if (t === "light") return "vs";
  if (t === "contraste") return "hc-black";
  return "regente-aurora";
}
