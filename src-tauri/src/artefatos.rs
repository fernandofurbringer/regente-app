//! Artefatos por referência (RM-19, H1-15): resultado grande vai para arquivo em
//! `<pasta do painel>/.regente/artefatos/<missão|_ws>/`. A entrega leva só caminho, formato, tamanho e resumo;
//! a leitura devolve só a fatia pedida (16 KB no máximo). Escopo: a missão do chamador ou `_ws`.
//! ponytail: CSV sem aspas (vírgula dentro de campo quebra a fatia por campo), add when um dado real precisar.
use crate::error::{fail, AppResult};
use crate::formato::{self, Formato};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Component, Path, PathBuf};

/// Teto de escrita e de leitura de arquivo de artefato.
pub const CAP_ESCRITA: usize = 5 * 1024 * 1024;
/// Teto da saída de `artifact_read`.
pub const CAP_LEITURA: usize = 16 * 1024;
pub const SCOPE_WS: &str = "_ws";
/// Linha do prompt do piloto (lida pelo `pilot_prompt`).
pub const REGRA_PILOTO: &str = "Entregue resultados grandes como artefato (caminho + resumo); leia artefatos com artifact_read pedindo só os campos/linhas necessários.";

/// O que a entrega registra sobre o arquivo. O conteúdo nunca entra na entrega.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artefato {
    /// Caminho relativo à pasta do painel.
    pub path: String,
    pub format: String,
    pub bytes: usize,
    pub rows: Option<usize>,
    pub fields: Option<Vec<String>>,
    pub summary: String,
}

/// Nome ou escopo: `[A-Za-z0-9._-]`, até 64, sem começar por ponto (nada de `..`).
fn nome_valido(s: &str) -> bool {
    s.len() <= 64 && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// Campos de uma descrição de artefato (a entrega não carrega mais que isso).
const CAMPOS_MAX: usize = 20;

fn pasta(cwd: &Path, scope: &str) -> PathBuf {
    cwd.join(".regente").join("artefatos").join(scope)
}

pub fn ext_de(f: Formato) -> &'static str {
    match f {
        Formato::Json => "json",
        Formato::Csv => "csv",
        Formato::Toon => "toon",
        Formato::Markdown => "md",
    }
}

fn formato_de_ext(ext: &str) -> AppResult<Formato> {
    Ok(match ext {
        "json" => Formato::Json,
        "csv" => Formato::Csv,
        "toon" => Formato::Toon,
        "md" => Formato::Markdown,
        _ => return fail("bad_format", "formato de artefato: auto, json, csv, toon ou md"),
    })
}

/// Formato pedido pelo agente (`auto` segue o perfil: `json` ou `md` fixam; senão `choose_format`).
pub fn escolher(pedido: Option<&str>, v: &Value, perfil: &str) -> AppResult<Formato> {
    Ok(match pedido.unwrap_or("auto") {
        "auto" => match perfil {
            "json" => Formato::Json,
            "md" => Formato::Markdown,
            _ => formato::choose_format(v),
        },
        outro => formato_de_ext(outro)?,
    })
}

/// Escreve o conteúdo já codificado (atômico: arquivo temporário e renomeação) e devolve a descrição.
/// Pasta do escopo, criada só se nenhum degrau (`.regente`, `artefatos`, escopo) for link, e confinada à raiz canônica.
fn pasta_segura(cwd: &Path, scope: &str) -> AppResult<PathBuf> {
    let dir = pasta(cwd, scope);
    for degrau in [cwd.join(".regente"), cwd.join(".regente").join("artefatos"), dir.clone()] {
        if std::fs::symlink_metadata(&degrau).is_ok_and(|m| crate::projeto::e_link(&m)) {
            return fail("regente_link", "pasta de artefatos é um link; o Regente não grava por ele.");
        }
    }
    std::fs::create_dir_all(&dir)?;
    let base = cwd.canonicalize()?;
    if !dir.canonicalize()?.starts_with(&base) {
        return fail("artifact_scope", "pasta de artefatos fora da pasta do painel");
    }
    Ok(dir)
}

pub fn write(cwd: &Path, scope: &str, name: &str, content: &str, format: &str) -> AppResult<Artefato> {
    if !nome_valido(name) || !nome_valido(scope) {
        return fail("bad_name", "nome de artefato: letras, números, ponto, traço ou sublinhado (até 64)");
    }
    let ext = ext_de(formato_de_ext(format)?);
    if content.len() > CAP_ESCRITA {
        return fail("artifact_too_big", "artefato acima de 5 MB");
    }
    let dir = pasta_segura(cwd, scope)?;
    let file = dir.join(format!("{name}.{ext}"));
    let tmp = dir.join(format!(".{name}.{ext}.tmp"));
    crate::projeto::escreve_novo(&tmp, content.as_bytes())?;
    std::fs::rename(&tmp, &file)?;
    Ok(descreve(format!(".regente/artefatos/{scope}/{name}.{ext}"), content, ext))
}

/// Completa linhas e campos a partir do valor gravado (o TOON não se lê de volta aqui).
pub fn com_valor(mut a: Artefato, v: &Value) -> Artefato {
    if let Value::Array(items) = v {
        a.rows = Some(items.len());
        a.fields = match items.first() {
            Some(Value::Object(m)) => Some(m.keys().take(CAMPOS_MAX).cloned().collect()),
            _ => None,
        };
    }
    a
}

/// Descreve um arquivo que o agente escreveu (`artifact: {path}`). Confere o escopo antes de ler.
pub fn registrar_existente(cwd: &Path, missao: Option<&str>, rel: &str) -> AppResult<Artefato> {
    let file = resolve(cwd, missao, rel)?;
    if std::fs::metadata(&file)?.len() as usize > CAP_ESCRITA {
        return fail("artifact_too_big", "artefato acima de 5 MB");
    }
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string();
    formato_de_ext(&ext)?;
    let text = std::fs::read_to_string(&file)?;
    Ok(descreve(rel.replace('\\', "/"), &text, &ext))
}

/// Caminho relativo a `cwd`, dentro de `.regente/artefatos/<missão|_ws>/`. Recusa absoluto, `..` e escopo alheio.
pub fn resolve(cwd: &Path, missao: Option<&str>, rel: &str) -> AppResult<PathBuf> {
    let p = Path::new(rel);
    let limpo = p.components().all(|c| matches!(c, Component::Normal(_)));
    let partes: Vec<String> = p.components().filter_map(|c| c.as_os_str().to_str().map(String::from)).collect();
    if !limpo || partes.len() != 4 || partes[0] != ".regente" || partes[1] != "artefatos" {
        return fail("artifact_scope", "caminho fora da pasta de artefatos");
    }
    let escopo = partes[2].as_str();
    if escopo != SCOPE_WS && Some(escopo) != missao {
        return fail("artifact_scope", "artefato de outra missão ou workspace");
    }
    let base = cwd.canonicalize()?;
    let Ok(real) = cwd.join(p).canonicalize() else { return fail("artifact_missing", "artefato não encontrado") };
    // o caminho real (sem links) tem que estar dentro do escopo pedido, não só dentro do painel
    if !real.starts_with(base.join(".regente").join("artefatos").join(escopo)) {
        return fail("artifact_scope", "caminho fora da pasta de artefatos do escopo");
    }
    Ok(real)
}

/// Fatia de um artefato: filtro (`query`, substring sem caixa), `offset`/`limit` sobre itens ou linhas, e
/// `fields` (JSON e CSV). `format` escolhe a saída (padrão: JSON para arrays, texto puro para o resto). Até 16 KB.
pub fn read_slice(path: &Path, fields: Option<&[String]>, offset: usize, limit: Option<usize>, query: Option<&str>, format: Option<&str>) -> AppResult<String> {
    if std::fs::metadata(path)?.len() as usize > CAP_ESCRITA {
        return fail("artifact_too_big", "artefato acima de 5 MB");
    }
    let text = std::fs::read_to_string(path)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let q = query.map(str::to_lowercase);
    let take = limit.unwrap_or(usize::MAX);
    let out = match (ext, serde_json::from_str::<Value>(&text)) {
        ("json", Ok(Value::Array(items))) => {
            let mut itens: Vec<Value> = items.into_iter().filter(|v| q.as_ref().is_none_or(|q| v.to_string().to_lowercase().contains(q))).skip(offset).take(take).collect();
            if let Some(c) = fields {
                itens = itens.into_iter().map(|v| projetar(v, c)).collect();
            }
            let v = Value::Array(itens);
            let f = match format {
                None | Some("json") => Formato::Json,
                Some(_) => escolher(format, &v, "")?,
            };
            formato::encode(&v, f)
        }
        ("csv", _) => {
            let mut linhas = text.lines();
            let cabecalho: Vec<&str> = linhas.next().map(|h| h.split(',').collect()).unwrap_or_default();
            let idx: Vec<usize> = match fields {
                Some(c) => c.iter().filter_map(|f| cabecalho.iter().position(|h| h == f)).collect(),
                None => (0..cabecalho.len()).collect(),
            };
            let linha = |l: &str| -> String {
                let cols: Vec<&str> = l.split(',').collect();
                idx.iter().filter_map(|&i| cols.get(i).copied()).collect::<Vec<_>>().join(",")
            };
            let corpo: Vec<String> = linhas.filter(|l| q.as_ref().is_none_or(|q| l.to_lowercase().contains(q))).skip(offset).take(take).map(linha).collect();
            let mut saida = vec![idx.iter().filter_map(|&i| cabecalho.get(i).copied()).collect::<Vec<_>>().join(",")];
            saida.extend(corpo);
            saida.join("\n")
        }
        // TOON, Markdown, JSON que não é array: fatia por linhas, sem campos
        _ => text.lines().filter(|l| q.as_ref().is_none_or(|q| l.to_lowercase().contains(q))).skip(offset).take(take).collect::<Vec<_>>().join("\n"),
    };
    Ok(limitar(out))
}

fn projetar(v: Value, campos: &[String]) -> Value {
    match v {
        Value::Object(m) => Value::Object(campos.iter().filter_map(|c| m.get(c).map(|x| (c.clone(), x.clone()))).collect::<Map<String, Value>>()),
        outro => outro,
    }
}

/// Corta na fronteira de caractere e avisa, para a resposta não passar de 16 KB.
fn limitar(mut s: String) -> String {
    if s.len() <= CAP_LEITURA {
        return s;
    }
    let mut corte = CAP_LEITURA;
    while !s.is_char_boundary(corte) {
        corte -= 1;
    }
    s.truncate(corte);
    s.push_str("\n[truncado em 16 KB: use offset/limit ou query para ler o resto]");
    s
}

fn descreve(rel: String, content: &str, ext: &str) -> Artefato {
    let (rows, fields) = estrutura(content, ext);
    let fields = fields.map(|f| f.into_iter().take(CAMPOS_MAX).collect::<Vec<_>>());
    Artefato { path: rel, format: ext.into(), bytes: content.len(), summary: resumo(content, rows, fields.as_deref()), rows, fields }
}
/// Linhas e campos de um array JSON de objetos ou de um CSV.
fn estrutura(content: &str, ext: &str) -> (Option<usize>, Option<Vec<String>>) {
    match ext {
        "json" => match serde_json::from_str::<Value>(content) {
            Ok(Value::Array(items)) => {
                let campos = match items.first() {
                    Some(Value::Object(m)) => Some(m.keys().cloned().collect()),
                    _ => None,
                };
                (Some(items.len()), campos)
            }
            _ => (None, None),
        },
        "csv" => {
            let mut linhas = content.lines();
            let campos = linhas.next().map(|h| h.split(',').map(String::from).collect());
            (Some(linhas.count()), campos)
        }
        _ => (None, None),
    }
}

/// Resumo curto: "N linhas; campos: a, b" quando há estrutura, senão a primeira linha. Até 120 caracteres.
fn resumo(content: &str, rows: Option<usize>, campos: Option<&[String]>) -> String {
    let s = match (rows, campos) {
        (Some(n), Some(c)) => format!("{n} linhas; campos: {}", c.join(", ")),
        (Some(n), None) => format!("{n} linhas"),
        _ => content.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string(),
    };
    s.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pasta_teste() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn write_then_describe_json_array() {
        let d = pasta_teste();
        let texto = r#"[{"id":1,"nome":"a"},{"id":2,"nome":"b"}]"#;
        let a = write(d.path(), "m1", "dados", texto, "json").unwrap();
        assert_eq!(a.path, ".regente/artefatos/m1/dados.json");
        assert_eq!((a.rows, a.bytes, a.format.as_str()), (Some(2), texto.len(), "json"));
        assert_eq!(a.fields.unwrap().len(), 2);
        assert!(a.summary.starts_with("2 linhas"));
    }

    #[test]
    fn names_and_scopes_are_checked() {
        let d = pasta_teste();
        for n in ["..", ".oculto", "a/b", "a b", ""] {
            assert!(write(d.path(), "m1", n, "x", "md").is_err(), "{n}");
        }
        assert!(write(d.path(), "../fora", "x", "x", "md").is_err());
        assert!(write(d.path(), "m1", "x", "x", "exe").is_err());
        assert!(write(d.path(), "m1", &"a".repeat(65), "x", "md").is_err());
    }

    #[test]
    fn write_refuses_linked_artifact_dirs() {
        let d = pasta_teste();
        let fora = pasta_teste();
        let alvo = fora.path().join("destino");
        std::fs::create_dir_all(&alvo).unwrap();
        std::fs::create_dir_all(d.path().join(".regente")).unwrap();
        let link = d.path().join(".regente").join("artefatos");
        #[cfg(windows)]
        let criou = std::os::windows::fs::symlink_dir(&alvo, &link).is_ok();
        #[cfg(unix)]
        let criou = std::os::unix::fs::symlink(&alvo, &link).is_ok();
        if !criou {
            eprintln!("symlink indisponível; teste de link pulado");
            return;
        }
        assert_eq!(write(d.path(), "m1", "x", "x", "md").unwrap_err().code(), "regente_link");
        assert_eq!(std::fs::read_dir(&alvo).unwrap().count(), 0, "nada é gravado fora da raiz");
    }

    #[test]
    fn write_refuses_over_five_megabytes() {
        let d = pasta_teste();
        let grande = "x".repeat(CAP_ESCRITA + 1);
        assert_eq!(write(d.path(), "m1", "g", &grande, "md").unwrap_err().code(), "artifact_too_big");
    }

    #[test]
    fn resolve_keeps_reads_inside_own_scope() {
        let d = pasta_teste();
        write(d.path(), "m1", "ok", "x", "md").unwrap();
        write(d.path(), "m2", "outra", "x", "md").unwrap();
        write(d.path(), SCOPE_WS, "geral", "x", "md").unwrap();
        assert!(resolve(d.path(), Some("m1"), ".regente/artefatos/m1/ok.md").is_ok());
        assert!(resolve(d.path(), Some("m1"), ".regente/artefatos/_ws/geral.md").is_ok());
        // outra missão, `..`, absoluto e fora da pasta de artefatos: recusados
        assert_eq!(resolve(d.path(), Some("m1"), ".regente/artefatos/m2/outra.md").unwrap_err().code(), "artifact_scope");
        assert_eq!(resolve(d.path(), Some("m1"), ".regente/artefatos/m1/../m2/outra.md").unwrap_err().code(), "artifact_scope");
        let abs = d.path().join(".regente/artefatos/m1/ok.md");
        assert_eq!(resolve(d.path(), Some("m1"), &abs.to_string_lossy()).unwrap_err().code(), "artifact_scope");
        assert_eq!(resolve(d.path(), Some("m1"), "segredo.txt").unwrap_err().code(), "artifact_scope");
        assert_eq!(resolve(d.path(), None, ".regente/artefatos/m1/ok.md").unwrap_err().code(), "artifact_scope");
    }

    #[test]
    fn slices_json_by_fields_query_and_range() {
        let d = pasta_teste();
        let texto = json!([{"id":1,"nome":"alfa","x":1},{"id":2,"nome":"beta","x":2},{"id":3,"nome":"Alfabeto","x":3}]).to_string();
        let a = write(d.path(), "m1", "j", &texto, "json").unwrap();
        let p = d.path().join(&a.path);
        let campos = vec!["nome".to_string()];
        let s = read_slice(&p, Some(&campos), 0, None, Some("ALFA"), None).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v, json!([{"nome": "alfa"}, {"nome": "Alfabeto"}]));
        let s = read_slice(&p, None, 1, Some(1), None, None).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap()[0]["id"], 2);
    }

    #[test]
    fn slices_csv_and_text_by_line() {
        let d = pasta_teste();
        let csv = "id,nome,x\n1,alfa,9\n2,beta,8\n3,gama,7";
        let a = write(d.path(), "m1", "c", csv, "csv").unwrap();
        assert_eq!(a.rows, Some(3));
        let p = d.path().join(&a.path);
        let campos = vec!["nome".to_string(), "id".to_string()];
        assert_eq!(read_slice(&p, Some(&campos), 0, Some(2), Some("e"), None).unwrap(), "nome,id\nbeta,2");
        let md = write(d.path(), "m1", "t", "linha um\nlinha dois\nterceira", "md").unwrap();
        let s = read_slice(&d.path().join(&md.path), None, 1, Some(1), None, None).unwrap();
        assert_eq!(s, "linha dois");
    }

    #[test]
    fn output_is_capped_at_sixteen_kilobytes() {
        let d = pasta_teste();
        let longo = "á".repeat(10_000);
        let a = write(d.path(), "m1", "grande", &longo, "md").unwrap();
        let s = read_slice(&d.path().join(&a.path), None, 0, None, None, None).unwrap();
        assert!(s.len() <= CAP_LEITURA + 80, "{}", s.len());
        assert!(s.contains("truncado"));
    }

    #[test]
    fn registrar_existente_reads_the_agents_own_file() {
        let d = pasta_teste();
        let dir = d.path().join(".regente/artefatos/m1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dele.json"), r#"[{"a":1}]"#).unwrap();
        let a = registrar_existente(d.path(), Some("m1"), ".regente/artefatos/m1/dele.json").unwrap();
        assert_eq!((a.rows, a.format.as_str()), (Some(1), "json"));
        assert!(registrar_existente(d.path(), Some("m1"), ".regente/artefatos/m1/nao-existe.json").is_err());
    }

    #[test]
    fn escolher_honors_profile_and_explicit_format() {
        let v = json!([{"a": 1}, {"a": 2}]);
        assert_eq!(escolher(None, &v, "auto").unwrap(), Formato::Toon);
        assert_eq!(escolher(Some("auto"), &v, "json").unwrap(), Formato::Json);
        assert_eq!(escolher(Some("csv"), &v, "json").unwrap(), Formato::Csv);
        assert!(escolher(Some("xml"), &v, "auto").is_err());
    }
}
