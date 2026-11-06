//! Símbolos de um arquivo com tree-sitter: tipo, nome, linhas e a linha da assinatura. Nunca devolve corpos.
//! Uma consulta por linguagem, compilada uma vez. `top` = declarado no nível do arquivo (fora de classe, impl ou bloco).

use std::ops::ControlFlow;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser, Point, Query, QueryCursor, StreamingIterator};

/// Assinatura guardada por símbolo (caracteres).
const SIG_MAX: usize = 200;
/// Tempo máximo para parsear um arquivo. Nos testes é folgado: com a suíte toda em paralelo, 200 ms estoura e o
/// outline sai truncado (o teste de tamanho falhava só sob carga).
const PARSE_MAX: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_millis(200) };
/// Nós que podem envolver uma declaração de nível superior.
const ENVOLTORIOS: &[&str] = &["source_file", "program", "module", "export_statement", "decorated_definition", "type_declaration"];

#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub name: String,
    /// function | struct | enum | trait | type | mod | const | static | class | interface | method
    pub kind: String,
    /// Linha 1-based da declaração.
    pub line: u32,
    pub end_line: u32,
    pub top: bool,
    /// Primeira linha da declaração, sem espaços das pontas, até 200 caracteres.
    pub sig: String,
}

struct Lingua {
    nome: &'static str,
    lang: Language,
    query: Query,
}

/// Linguagem pela extensão (sem ponto). None = sem índice.
pub fn lingua_da_extensao(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "py" => Some("python"),
        "go" => Some("go"),
        _ => None,
    }
}

const RUST: &str = "
(function_item name: (identifier) @name) @function
(struct_item name: (type_identifier) @name) @struct
(enum_item name: (type_identifier) @name) @enum
(trait_item name: (type_identifier) @name) @trait
(type_item name: (type_identifier) @name) @type
(mod_item name: (identifier) @name) @mod
(const_item name: (identifier) @name) @const
(static_item name: (identifier) @name) @static
";

const TS: &str = "
(function_declaration name: (identifier) @name) @function
(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @function
(class_declaration name: (type_identifier) @name) @class
(interface_declaration name: (type_identifier) @name) @interface
(type_alias_declaration name: (type_identifier) @name) @type
(enum_declaration name: (identifier) @name) @enum
(method_definition name: (property_identifier) @name) @method
";

const JS: &str = "
(function_declaration name: (identifier) @name) @function
(lexical_declaration (variable_declarator name: (identifier) @name value: (arrow_function))) @function
(class_declaration name: (identifier) @name) @class
(method_definition name: (property_identifier) @name) @method
";

const PY: &str = "
(function_definition name: (identifier) @name) @function
(class_definition name: (identifier) @name) @class
";

const GO: &str = "
(function_declaration name: (identifier) @name) @function
(method_declaration name: (field_identifier) @name) @method
(type_spec name: (type_identifier) @name) @type
";

fn linguas() -> &'static [Lingua] {
    static LINGUAS: OnceLock<Vec<Lingua>> = OnceLock::new();
    LINGUAS.get_or_init(|| {
        let ts: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let tsx: Language = tree_sitter_typescript::LANGUAGE_TSX.into();
        let defs: Vec<(&'static str, Language, &str)> = vec![
            ("rust", tree_sitter_rust::LANGUAGE.into(), RUST),
            ("typescript", ts, TS),
            ("tsx", tsx, TS),
            ("javascript", tree_sitter_javascript::LANGUAGE.into(), JS),
            ("python", tree_sitter_python::LANGUAGE.into(), PY),
            ("go", tree_sitter_go::LANGUAGE.into(), GO),
        ];
        defs.into_iter().filter_map(|(nome, lang, src)| match Query::new(&lang, src) {
            Ok(query) => Some(Lingua { nome, lang, query }),
            Err(e) => {
                tracing::warn!("consulta de {nome} não compila: {e}");
                None
            }
        }).collect()
    })
}

/// Símbolos do texto, por linha. Linguagem desconhecida ou texto que o parser não lê = vazio.
pub fn simbolos(nome: &str, texto: &str) -> Vec<Symbol> {
    let Some(l) = linguas().iter().find(|l| l.nome == nome) else { return vec![] };
    let mut p = Parser::new();
    if p.set_language(&l.lang).is_err() {
        return vec![];
    }
    let fonte = texto.as_bytes();
    // arquivo que trava o parser (ex.: gerado, minificado) é pulado; o log guarda o motivo
    let inicio = Instant::now();
    let mut ate = |_: &ParseState| if inicio.elapsed() > PARSE_MAX { ControlFlow::Break(()) } else { ControlFlow::Continue(()) };
    let mut ler = |i: usize, _: Point| -> &[u8] { if i < fonte.len() { &fonte[i..] } else { &[] } };
    let Some(arvore) = p.parse_with_options(&mut ler, None, Some(ParseOptions::new().progress_callback(&mut ate))) else {
        tracing::warn!("codeindex: {nome} passou de {PARSE_MAX:?} ao ler; arquivo pulado");
        return vec![];
    };
    let linhas: Vec<&str> = texto.lines().collect();
    let nomes = l.query.capture_names();
    let mut out = vec![];
    let mut cursor = QueryCursor::new();
    let mut it = cursor.matches(&l.query, arvore.root_node(), fonte);
    while let Some(m) = it.next() {
        let (mut nome_no, mut def) = (None, None);
        for c in m.captures() {
            let cap = nomes[c.index as usize];
            if cap == "name" {
                nome_no = Some(c.node);
            } else {
                def = Some((cap, c.node));
            }
        }
        let (Some(no), Some((kind, d))) = (nome_no, def) else { continue };
        let Ok(name) = no.utf8_text(fonte) else { continue };
        let linha0 = d.start_position().row;
        let sig: String = linhas.get(linha0).map_or("", |s| s.trim()).chars().take(SIG_MAX).collect();
        out.push(Symbol { name: name.to_string(), kind: kind.to_string(), line: linha0 as u32 + 1, end_line: d.end_position().row as u32 + 1, top: no_topo(d), sig });
    }
    out.sort_by_key(|s| s.line);
    out
}

/// Nível do arquivo: só envoltórios de declaração acima do nó (classe, impl, bloco ou função deixam de ser topo).
fn no_topo(n: Node) -> bool {
    let mut a = n.parent();
    while let Some(p) = a {
        if !ENVOLTORIOS.contains(&p.kind()) {
            return false;
        }
        a = p.parent();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nomes(s: &[Symbol]) -> Vec<(String, String, u32)> {
        s.iter().map(|x| (x.kind.clone(), x.name.clone(), x.line)).collect()
    }

    #[test]
    fn rust_top_level_names_and_lines_are_exact() {
        let src = "use std::fmt;\n\npub struct Pessoa {\n    nome: String,\n}\n\nimpl Pessoa {\n    pub fn novo() -> Self { todo!() }\n}\n\nfn principal() {\n    let x = 1;\n}\n\nconst MAX: u32 = 3;\n";
        let s = simbolos("rust", src);
        assert_eq!(nomes(&s), vec![("struct".into(), "Pessoa".into(), 3), ("function".into(), "novo".into(), 8), ("function".into(), "principal".into(), 11), ("const".into(), "MAX".into(), 15)]);
        let top: Vec<&str> = s.iter().filter(|x| x.top).map(|x| x.name.as_str()).collect();
        assert_eq!(top, vec!["Pessoa", "principal", "MAX"], "método dentro de impl não é topo");
        assert_eq!(s[0].sig, "pub struct Pessoa {");
    }

    #[test]
    fn typescript_exports_arrows_and_classes() {
        let src = "export function soma(a: number): number {\n  return a;\n}\n\nexport const dobro = (x: number) => x * 2;\n\nexport class Caixa {\n  abrir(): void {}\n}\n\ninterface Forma { area: number }\n";
        let s = simbolos("typescript", src);
        assert_eq!(nomes(&s), vec![("function".into(), "soma".into(), 1), ("function".into(), "dobro".into(), 5), ("class".into(), "Caixa".into(), 7), ("method".into(), "abrir".into(), 8), ("interface".into(), "Forma".into(), 11)]);
        assert!(!s.iter().find(|x| x.name == "abrir").unwrap().top);
    }

    #[test]
    fn javascript_and_python_fixtures() {
        let js = "function a() {}\nclass B {\n  c() {}\n}\nconst d = () => 1;\n";
        assert_eq!(nomes(&simbolos("javascript", js)), vec![("function".into(), "a".into(), 1), ("class".into(), "B".into(), 2), ("method".into(), "c".into(), 3), ("function".into(), "d".into(), 5)]);
        let py = "import os\n\n@cache\ndef ler(p):\n    return p\n\nclass Modelo:\n    def salvar(self):\n        pass\n";
        let s = simbolos("python", py);
        assert_eq!(nomes(&s), vec![("function".into(), "ler".into(), 4), ("class".into(), "Modelo".into(), 7), ("function".into(), "salvar".into(), 8)]);
        assert_eq!(s.iter().filter(|x| x.top).count(), 2, "método de classe não é topo");
    }

    #[test]
    fn go_fixture_with_method_and_type() {
        let go = "package main\n\ntype Servidor struct {\n\tporta int\n}\n\nfunc (s *Servidor) Ligar() {}\n\nfunc main() {}\n";
        let s = simbolos("go", go);
        assert_eq!(nomes(&s), vec![("type".into(), "Servidor".into(), 3), ("method".into(), "Ligar".into(), 7), ("function".into(), "main".into(), 9)]);
        assert!(s.iter().all(|x| x.top), "métodos Go são declarados no topo do arquivo");
    }

    #[test]
    fn unknown_language_is_empty() {
        assert!(simbolos("cobol", "x").is_empty());
        assert_eq!(lingua_da_extensao("TSX"), Some("tsx"));
        assert_eq!(lingua_da_extensao("md"), None);
    }

    /// Outline em texto (uma linha por símbolo de topo) tem que caber em menos de 10% dos tokens do arquivo (chars/4).
    #[test]
    fn outline_is_under_ten_percent_of_the_file() {
        let mut src = String::new();
        for i in 0..60 {
            src.push_str(&format!("pub fn funcao_{i}(x: u32) -> u32 {{\n"));
            for j in 0..25 {
                src.push_str(&format!("    let v{j} = x.wrapping_mul({j}) + {i} * 3; // corpo longo\n"));
            }
            src.push_str("    x\n}\n\n");
        }
        let texto: String = simbolos("rust", &src).iter().filter(|x| x.top).map(|x| format!("{} {} L{}-{}\n", x.kind, x.name, x.line, x.end_line)).collect();
        assert_eq!(simbolos("rust", &src).iter().filter(|x| x.top).count(), 60);
        assert!(texto.len() / 4 * 10 < src.len() / 4, "outline {} tokens, arquivo {} tokens", texto.len() / 4, src.len() / 4);
    }
}
