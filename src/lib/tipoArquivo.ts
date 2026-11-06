/** Selo do tipo de arquivo na árvore e nas abas do editor (rótulo curto e classe de cor), no lugar dos ícones do VS Code. */
export interface TipoArquivo { rotulo: string; classe: string }

const t = (rotulo: string, classe: string): TipoArquivo => ({ rotulo, classe });
const POR_EXT: Record<string, TipoArquivo> = {
  ts: t("TS", "ts"), tsx: t("TSX", "ts"), mts: t("TS", "ts"), cts: t("TS", "ts"),
  js: t("JS", "js"), jsx: t("JSX", "js"), mjs: t("JS", "js"), cjs: t("JS", "js"),
  json: t("{}", "json"), jsonc: t("{}", "json"),
  rs: t("RS", "rs"), py: t("PY", "py"), go: t("GO", "go"), java: t("JV", "java"), kt: t("KT", "java"), cs: t("C#", "cs"),
  c: t("C", "c"), h: t("H", "c"), cpp: t("C++", "c"), hpp: t("H++", "c"),
  md: t("MD", "md"), mdx: t("MDX", "md"), txt: t("TXT", "outro"),
  css: t("CSS", "css"), scss: t("SCSS", "css"), less: t("LESS", "css"),
  html: t("<>", "html"), htm: t("<>", "html"), vue: t("VUE", "vue"), svelte: t("SV", "html"),
  yml: t("YML", "yml"), yaml: t("YML", "yml"), toml: t("TML", "yml"), ini: t("INI", "yml"),
  sql: t("SQL", "sql"), sh: t("SH", "sh"), ps1: t("PS", "sh"), bat: t("BAT", "sh"),
  png: t("IMG", "img"), jpg: t("IMG", "img"), jpeg: t("IMG", "img"), gif: t("IMG", "img"), svg: t("SVG", "img"), webp: t("IMG", "img"), ico: t("ICO", "img"),
  lock: t("LCK", "outro"), log: t("LOG", "outro"),
};
const POR_NOME: Record<string, TipoArquivo> = {
  "package.json": t("NPM", "npm"), "cargo.toml": t("CRG", "rs"), "cargo.lock": t("CRG", "rs"),
  dockerfile: t("DKR", "docker"), ".gitignore": t("GIT", "git"), ".gitattributes": t("GIT", "git"),
  "tsconfig.json": t("TS", "ts"), "readme.md": t("i", "info"), license: t("LIC", "outro"),
};

export function tipoArquivo(nome: string): TipoArquivo {
  const n = nome.toLowerCase();
  if (POR_NOME[n]) return POR_NOME[n];
  if (n === ".env" || n.startsWith(".env.")) return t("ENV", "env");
  const ponto = n.lastIndexOf(".");
  return (ponto > 0 && POR_EXT[n.slice(ponto + 1)]) || t("·", "outro");
}
