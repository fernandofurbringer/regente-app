//! Formato compacto automático para artefatos e saídas (H1-16, parte pura).
//! `choose_format` escolhe o formato pelo formato do dado: array uniforme de objetos planos vira TOON (ou CSV
//! quando TOON está desligado), o resto é JSON minificado, e texto vai como Markdown. Sem `TOON_ON` ligado o
//! encoder TOON cai em JSON, que é sempre decodificável.
//!
//! TOON segue o SPEC de github.com/toon-format/spec (cabeçalho `[N]{campos}:`, linhas de 2 espaços, citação de
//! strings conforme §7.2). Só o subconjunto que o Regente emite: objetos, arrays de escalares e arrays tabulares.
//! Tudo fora disso cai em JSON. ponytail: sem list-form, add when um dado misto precisar de TOON.
use serde_json::{Map, Value};

/// Interruptor do TOON (risco R-18). Desligado, o array uniforme vai como CSV.
pub const TOON_ON: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formato {
    Toon,
    Csv,
    Json,
    Markdown,
}

pub fn choose_format(v: &Value) -> Formato {
    escolher(v, TOON_ON)
}

fn escolher(v: &Value, toon_on: bool) -> Formato {
    match v {
        Value::String(_) => Formato::Markdown,
        Value::Array(items) if uniforme_plano(items) => {
            if toon_on { Formato::Toon } else { Formato::Csv }
        }
        _ => Formato::Json,
    }
}

/// Array não vazio de objetos com as mesmas chaves e só valores escalares.
fn uniforme_plano(items: &[Value]) -> bool {
    let Some(Value::Object(primeiro)) = items.first() else { return false };
    items.iter().all(|it| match it {
        Value::Object(m) => {
            m.len() == primeiro.len()
                && m.iter().all(|(k, v)| primeiro.contains_key(k) && !v.is_object() && !v.is_array())
        }
        _ => false,
    })
}

pub fn encode(v: &Value, formato: Formato) -> String {
    match formato {
        Formato::Markdown => match v {
            Value::String(s) => s.clone(),
            outro => json(outro),
        },
        Formato::Json => json(v),
        Formato::Csv => match v {
            Value::Array(items) if uniforme_plano(items) => csv(items),
            outro => json(outro),
        },
        Formato::Toon => toon(v).unwrap_or_else(|| json(v)),
    }
}

/// Formato que `encode` usa de fato: cai em JSON quando o pedido não cabe no dado.
pub fn efetivo(v: &Value, f: Formato) -> Formato {
    match (f, v) {
        (Formato::Markdown, Value::String(_)) => f,
        (Formato::Markdown | Formato::Json, _) => Formato::Json,
        (Formato::Csv, Value::Array(items)) if uniforme_plano(items) => f,
        (Formato::Csv, _) => Formato::Json,
        (Formato::Toon, _) if toon(v).is_some() => f,
        (Formato::Toon, _) => Formato::Json,
    }
}

/// Estimativa de tokens (proxy de chars/4).
pub fn est_tokens(s: &str) -> usize {
    s.chars().count() / 4
}

fn json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

// ---------- CSV (RFC 4180) ----------

fn csv(items: &[Value]) -> String {
    let Some(Value::Object(primeiro)) = items.first() else { return String::new() };
    let campos: Vec<&String> = primeiro.keys().collect();
    let mut linhas: Vec<String> = Vec::with_capacity(items.len() + 1);
    linhas.push(campos.iter().map(|c| csv_campo(c)).collect::<Vec<_>>().join(","));
    for it in items {
        if let Value::Object(m) = it {
            let celulas: Vec<String> = campos
                .iter()
                .map(|c| csv_campo(&m.get(*c).map(valor_csv).unwrap_or_default()))
                .collect();
            linhas.push(celulas.join(","));
        }
    }
    linhas.join("\r\n")
}

fn valor_csv(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        outro => outro.to_string(),
    }
}

fn csv_campo(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---------- TOON ----------

fn toon(v: &Value) -> Option<String> {
    let mut linhas: Vec<String> = Vec::new();
    match v {
        Value::Array(items) => array_lines("", items, 0, &mut linhas)?,
        Value::Object(m) => obj_lines(m, 0, &mut linhas)?,
        _ => return None,
    }
    Some(linhas.join("\n"))
}

fn ind(d: usize) -> String {
    "  ".repeat(d)
}

fn obj_lines(m: &Map<String, Value>, d: usize, out: &mut Vec<String>) -> Option<()> {
    for (k, v) in m {
        let chave = toon_str(k);
        match v {
            Value::Object(sub) => {
                out.push(format!("{}{chave}:", ind(d)));
                obj_lines(sub, d + 1, out)?;
            }
            Value::Array(items) => array_lines(&chave, items, d, out)?,
            escalar => out.push(format!("{}{chave}: {}", ind(d), prim(escalar)?)),
        }
    }
    Some(())
}

/// `chave` já vem citada, ou vazia para a raiz.
fn array_lines(chave: &str, items: &[Value], d: usize, out: &mut Vec<String>) -> Option<()> {
    if items.is_empty() {
        out.push(if chave.is_empty() { format!("{}[]", ind(d)) } else { format!("{}{chave}: []", ind(d)) });
        return Some(());
    }
    if items.iter().all(|x| !x.is_object() && !x.is_array()) {
        let vals: Vec<String> = items.iter().map(prim).collect::<Option<_>>()?;
        out.push(format!("{}{chave}[{}]: {}", ind(d), items.len(), vals.join(",")));
        return Some(());
    }
    if uniforme_plano(items) {
        let Value::Object(primeiro) = &items[0] else { return None };
        let campos: Vec<&String> = primeiro.keys().collect();
        let cab: Vec<String> = campos.iter().map(|c| toon_str(c)).collect();
        out.push(format!("{}{chave}[{}]{{{}}}:", ind(d), items.len(), cab.join(",")));
        for it in items {
            let Value::Object(m) = it else { return None };
            let celulas: Vec<String> = campos.iter().map(|c| prim(m.get(*c)?)).collect::<Option<_>>()?;
            out.push(format!("{}{}", ind(d + 1), celulas.join(",")));
        }
        return Some(());
    }
    None
}

fn prim(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => toon_str(s),
        _ => return None,
    })
}

fn toon_str(s: &str) -> String {
    if precisa_citar(s) { format!("\"{}\"", escapar(s)) } else { s.to_string() }
}

/// Regras de §7.2 do SPEC TOON. Na dúvida, cita: string citada sempre decodifica como string.
fn precisa_citar(s: &str) -> bool {
    s.is_empty()
        || s.starts_with([' ', '\t'])
        || s.ends_with([' ', '\t'])
        || matches!(s, "true" | "false" | "null")
        || parece_numero(s)
        || s.chars().any(|c| matches!(c, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | ',' | '\u{0}'..='\u{1f}'))
        || s.starts_with(['-', '#', '\u{feff}'])
}

fn parece_numero(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '.') || s.parse::<f64>().is_ok()
}

fn escapar(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Decodificador do subconjunto TOON que `encode` emite. Só para os testes de ida e volta.
    mod dec {
        use serde_json::{Map, Value};

        pub fn decode(text: &str) -> Option<Value> {
            let ls: Vec<(usize, &str)> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let sp = l.len() - l.trim_start_matches(' ').len();
                    (sp / 2, &l[sp..])
                })
                .collect();
            if ls.is_empty() {
                return Some(Value::Object(Map::new()));
            }
            let mut i = 0;
            if ls[0].1.starts_with('[') {
                i = 1; // o cabeçalho já foi lido
                let v = arr(&ls, &mut i, 0, &ls[0].1[1..])?;
                return (i == ls.len()).then_some(v);
            }
            let m = obj(&ls, &mut i, 0)?;
            (i == ls.len()).then_some(Value::Object(m))
        }

        fn obj(ls: &[(usize, &str)], i: &mut usize, d: usize) -> Option<Map<String, Value>> {
            let mut m = Map::new();
            while *i < ls.len() && ls[*i].0 == d {
                let linha = ls[*i].1;
                *i += 1;
                let (chave, resto) = chave(linha)?;
                if let Some(r) = resto.strip_prefix('[') {
                    m.insert(chave, arr(ls, i, d, r)?);
                } else if let Some(r) = resto.strip_prefix(':') {
                    if r.is_empty() {
                        let sub = if *i < ls.len() && ls[*i].0 == d + 1 { obj(ls, i, d + 1)? } else { Map::new() };
                        m.insert(chave, Value::Object(sub));
                    } else if r == " []" {
                        m.insert(chave, Value::Array(vec![]));
                    } else {
                        m.insert(chave, escalar(r.strip_prefix(' ')?)?);
                    }
                } else {
                    return None;
                }
            }
            Some(m)
        }

        /// `r` começa logo depois do `[` de abertura.
        fn arr(ls: &[(usize, &str)], i: &mut usize, d: usize, r: &str) -> Option<Value> {
            let fim = r.find(']')?;
            let n: usize = r[..fim].parse().ok()?;
            let resto = &r[fim + 1..];
            if let Some(c) = resto.strip_prefix('{') {
                let cab_fim = c.find("}:")?;
                let campos: Vec<String> = dividir(&c[..cab_fim]).iter().map(|t| unq(t)).collect::<Option<_>>()?;
                let mut itens = Vec::new();
                for _ in 0..n {
                    let (dd, linha) = *ls.get(*i)?;
                    if dd != d + 1 {
                        return None;
                    }
                    *i += 1;
                    let toks = dividir(linha);
                    if toks.len() != campos.len() {
                        return None;
                    }
                    let mut o = Map::new();
                    for (k, t) in campos.iter().zip(toks.iter()) {
                        o.insert(k.clone(), escalar(t)?);
                    }
                    itens.push(Value::Object(o));
                }
                return Some(Value::Array(itens));
            }
            let valores = resto.strip_prefix(": ").or(if n == 0 { Some("") } else { None })?;
            if n == 0 {
                return Some(Value::Array(vec![]));
            }
            let itens: Vec<Value> = dividir(valores).iter().map(|t| escalar(t)).collect::<Option<_>>()?;
            (itens.len() == n).then_some(Value::Array(itens))
        }

        /// Chave citada ou simples, e o resto da linha a partir de `[` ou `:`.
        fn chave(linha: &str) -> Option<(String, &str)> {
            if linha.starts_with('"') {
                let (s, rest) = citada(linha)?;
                return Some((s, rest));
            }
            let p = linha.find(['[', ':'])?;
            Some((linha[..p].to_string(), &linha[p..]))
        }

        fn citada(s: &str) -> Option<(String, &str)> {
            let mut it = s[1..].char_indices();
            let mut out = String::new();
            while let Some((idx, c)) = it.next() {
                match c {
                    '"' => return Some((out, &s[1 + idx + 1..])),
                    '\\' => {
                        let (_, e) = it.next()?;
                        match e {
                            'n' => out.push('\n'),
                            'r' => out.push('\r'),
                            't' => out.push('\t'),
                            '\\' => out.push('\\'),
                            '"' => out.push('"'),
                            'u' => {
                                let hex: String = (0..4).map(|_| it.next().map(|x| x.1)).collect::<Option<_>>()?;
                                out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                            }
                            _ => return None,
                        }
                    }
                    c => out.push(c),
                }
            }
            None
        }

        fn unq(t: &str) -> Option<String> {
            if t.starts_with('"') {
                let (s, rest) = citada(t)?;
                rest.is_empty().then_some(s)
            } else {
                Some(t.to_string())
            }
        }

        /// Separa por vírgula fora de aspas. Devolve os tokens brutos (com aspas).
        fn dividir(s: &str) -> Vec<String> {
            let mut out = Vec::new();
            let mut atual = String::new();
            let mut em_aspas = false;
            let mut esc = false;
            for c in s.chars() {
                if esc {
                    atual.push(c);
                    esc = false;
                } else if em_aspas && c == '\\' {
                    atual.push(c);
                    esc = true;
                } else if c == '"' {
                    em_aspas = !em_aspas;
                    atual.push(c);
                } else if c == ',' && !em_aspas {
                    out.push(std::mem::take(&mut atual));
                } else {
                    atual.push(c);
                }
            }
            out.push(atual);
            out
        }

        fn escalar(t: &str) -> Option<Value> {
            if t.starts_with('"') {
                return unq(t).map(Value::String);
            }
            match t {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                "null" => Some(Value::Null),
                _ => match serde_json::from_str::<Value>(t) {
                    Ok(n @ Value::Number(_)) => Some(n),
                    _ => Some(Value::String(t.to_string())),
                },
            }
        }
    }

    /// Decodificador CSV RFC 4180 (aspas duplicadas, quebras dentro de aspas).
    fn parse_csv(s: &str) -> Vec<Vec<String>> {
        let mut linhas = Vec::new();
        let mut linha = Vec::new();
        let mut campo = String::new();
        let mut aspas = false;
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if aspas {
                if c == '"' {
                    if it.peek() == Some(&'"') {
                        campo.push('"');
                        it.next();
                    } else {
                        aspas = false;
                    }
                } else {
                    campo.push(c);
                }
            } else if c == '"' {
                aspas = true;
            } else if c == ',' {
                linha.push(std::mem::take(&mut campo));
            } else if c == '\r' && it.peek() == Some(&'\n') {
                it.next();
                linha.push(std::mem::take(&mut campo));
                linhas.push(std::mem::take(&mut linha));
            } else {
                campo.push(c);
            }
        }
        if !campo.is_empty() || !linha.is_empty() {
            linha.push(campo);
            linhas.push(linha);
        }
        linhas
    }

    fn dados_teste() -> Vec<Value> {
        vec![
            json!({"id": 1, "nome": "Ada, Lovelace", "ok": true, "nota": 9.5}),
            json!({"id": 2, "nome": "Bob \"B\" Smith", "ok": false, "nota": null}),
            json!({"id": 3, "nome": "linha\núnica\ntexto", "ok": true, "nota": 0}),
            json!({"id": 4, "nome": " espaços nas pontas ", "ok": false, "nota": -1}),
            json!({"id": 5, "nome": "ação ✓ 日本語 🚀", "ok": true, "nota": 2}),
            json!({"id": 6, "nome": "true", "ok": false, "nota": 3}),
            json!({"id": 7, "nome": "42", "ok": true, "nota": 4}),
            json!({"id": 8, "nome": "a:b [x] {y}", "ok": false, "nota": 5}),
            json!({"id": 9, "nome": "tab\there\\barra", "ok": true, "nota": 6}),
            json!({"id": 10, "nome": "", "ok": false, "nota": 7}),
            json!({"id": 11, "nome": "-traço", "ok": true, "nota": 8}),
        ]
    }

    #[test]
    fn escolhe_formato_pela_forma_do_dado() {
        assert_eq!(choose_format(&Value::Array(dados_teste())), Formato::Toon);
        assert_eq!(escolher(&Value::Array(dados_teste()), false), Formato::Csv);
        assert_eq!(choose_format(&json!("texto livre")), Formato::Markdown);
        assert_eq!(choose_format(&json!({"a": {"b": 1}})), Formato::Json);
        assert_eq!(choose_format(&json!([{"a": 1}, {"a": 2, "b": 3}])), Formato::Json, "chaves diferentes");
        assert_eq!(choose_format(&json!([{"a": 1}, {"a": [1]}])), Formato::Json, "valor aninhado");
        assert_eq!(choose_format(&json!([1, 2, 3])), Formato::Json, "escalares soltos");
        assert_eq!(choose_format(&json!([])), Formato::Json);
        assert_eq!(choose_format(&json!(42)), Formato::Json);
    }

    #[test]
    fn toon_de_array_uniforme_segue_o_spec() {
        let v = json!([{"id": 1, "name": "Ada", "role": "admin"}, {"id": 2, "name": "Bob", "role": "user"}]);
        assert_eq!(encode(&v, Formato::Toon), "[2]{id,name,role}:\n  1,Ada,admin\n  2,Bob,user");
        let obj = json!({"users": [{"id": 1, "name": "Ada"}], "total": 1, "meta": {"v": true}});
        assert_eq!(
            encode(&obj, Formato::Toon),
            "meta:\n  v: true\ntotal: 1\nusers[1]{id,name}:\n  1,Ada"
        );
    }

    #[test]
    fn toon_cita_strings_conforme_o_spec() {
        let v = json!([{"a": "x,y", "b": "true", "c": "07", "d": "k: v", "e": "", "f": "-x", "g": "a\"b", "h": "n\nl"}]);
        assert_eq!(
            encode(&v, Formato::Toon),
            "[1]{a,b,c,d,e,f,g,h}:\n  \"x,y\",\"true\",\"07\",\"k: v\",\"\",\"-x\",\"a\\\"b\",\"n\\nl\""
        );
        assert_eq!(toon_str("café"), "café", "unicode passa direto");
        assert_eq!(toon_str("\u{1}"), "\"\\u0001\"", "controle vira \\u minúsculo");
    }

    #[test]
    fn toon_e_decodificavel_para_dados_dificeis() {
        let v = Value::Array(dados_teste());
        let t = encode(&v, Formato::Toon);
        assert_eq!(dec::decode(&t), Some(v), "texto:\n{t}");
    }

    #[test]
    fn toon_round_trip_de_objetos_aninhados() {
        let v = json!({
            "titulo": "Relatório: Q3, parte 1",
            "vazio": [],
            "vazio_obj": {},
            "lista": [1, "dois", true, null],
            "sub": {"a": {"b": "c d"}, "n": -2.5},
            "linhas": [{"k": "a", "v": 1}, {"k": "b", "v": 2}],
        });
        let t = encode(&v, Formato::Toon);
        assert_eq!(dec::decode(&t), Some(v), "texto:\n{t}");
    }

    #[test]
    fn toon_cai_em_json_fora_do_subconjunto() {
        let v = json!([1, {"a": 1}]);
        assert_eq!(encode(&v, Formato::Toon), serde_json::to_string(&v).unwrap());
    }

    #[test]
    fn csv_segue_rfc4180_e_e_decodificavel() {
        let v = Value::Array(dados_teste());
        let csv_txt = encode(&v, Formato::Csv);
        let linhas = parse_csv(&csv_txt);
        assert_eq!(linhas[0], ["id", "nome", "nota", "ok"]);
        assert_eq!(linhas.len(), 12);
        assert_eq!(linhas[1], ["1", "Ada, Lovelace", "9.5", "true"]);
        assert_eq!(linhas[2], ["2", "Bob \"B\" Smith", "", "false"]);
        assert_eq!(linhas[3], ["3", "linha\núnica\ntexto", "0", "true"]);
        assert_eq!(linhas[5], ["5", "ação ✓ 日本語 🚀", "2", "true"]);
        assert_eq!(linhas[10], ["10", "", "7", "false"]);
        assert!(csv_txt.contains("\r\n"));
    }

    #[test]
    fn csv_cita_so_quando_preciso() {
        assert_eq!(csv_campo("simples"), "simples");
        assert_eq!(csv_campo("a,b"), "\"a,b\"");
        assert_eq!(csv_campo("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_campo("a\nb"), "\"a\nb\"");
    }

    #[test]
    fn texto_sai_puro_em_markdown() {
        assert_eq!(encode(&json!("# título\n- item"), Formato::Markdown), "# título\n- item");
        assert_eq!(encode(&json!({"a": 1}), Formato::Markdown), "{\"a\":1}");
    }

    #[test]
    fn toon_e_csv_gastam_menos_tokens_que_json_em_50_linhas() {
        let linhas: Vec<Value> = (0..50)
            .map(|i| json!({"id": i, "titulo": format!("Tarefa número {i}"), "status": "aberta", "prioridade": i % 3}))
            .collect();
        let v = Value::Array(linhas);
        let j = est_tokens(&encode(&v, Formato::Json));
        let t = est_tokens(&encode(&v, Formato::Toon));
        let c = est_tokens(&encode(&v, Formato::Csv));
        assert!(t < j, "toon {t} vs json {j}");
        assert!(c < j, "csv {c} vs json {j}");
        assert_eq!(est_tokens("abcdefgh"), 2);
    }
}
