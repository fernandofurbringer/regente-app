use crate::error::{fail, AppError, AppResult};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const IGNORED: &[&str] = &[".git", "node_modules", "target", "dist", ".regente", ".next", "__pycache__", ".venv"];
pub const MAX_TEXT: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileText {
    pub text: String,
    pub mtime_ms: i64,
}

fn mtime_ms(p: &Path) -> AppResult<i64> {
    let m = std::fs::metadata(p)?.modified()?;
    Ok(m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0))
}

pub fn list_dir(dir: &Path) -> AppResult<Vec<FsEntry>> {
    list_dir_opts(dir, false)
}

/// `ignorados`: mostra também `.git`, `node_modules`, `target`… (a árvore tem o botão "mostrar ignorados").
pub fn list_dir_opts(dir: &Path, ignorados: bool) -> AppResult<Vec<FsEntry>> {
    let mut v: Vec<FsEntry> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !ignorados && IGNORED.contains(&name.as_str()) {
                return None;
            }
            // o tipo vem do próprio read_dir (sem um stat por item); link segue o alvo, como antes
            let is_dir = match e.file_type() {
                Ok(t) if t.is_symlink() => e.path().is_dir(),
                Ok(t) => t.is_dir(),
                Err(_) => e.path().is_dir(),
            };
            Some(FsEntry { path: e.path().to_string_lossy().to_string(), name, is_dir })
        })
        .collect();
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub rel: String,
    pub path: String,
}

/// Teto da lista do Ctrl+P e do `@` do compositor; acima dele a lista vem `truncated`.
pub const INDEX_MAX: usize = 20_000;
const INDEX_DIRS: usize = 500;
// ponytail: fora de git o cache só checa a mtime da raiz (e 60 s); pastas fundo podem ficar defasadas até expirar. Upgrade: watcher (notify).
const INDEX_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub files: Vec<IndexEntry>,
    /// Passou do teto (`INDEX_MAX`) ou das pastas: a lista é parcial.
    pub truncated: bool,
}

type IndexCache = BTreeMap<String, (i64, Instant, Index)>;
static INDEX: Mutex<IndexCache> = Mutex::new(BTreeMap::new());

/// Arquivos do workspace para o Ctrl+P: uma varredura por raiz, reaproveitada enquanto a mtime da raiz não mudar.
/// Em repositório git usa `git ls-files` (respeita o .gitignore); fora dele, a caminhada de pastas.
pub fn index(root: &Path) -> AppResult<Index> {
    let key = root.to_string_lossy().to_string();
    let mt = mtime_ms(root)?;
    if let Some((m, at, v)) = crate::util::lk(&INDEX).get(&key) {
        if *m == mt && at.elapsed() < INDEX_TTL {
            return Ok(v.clone());
        }
    }
    let v = match git_index(root) {
        Some(v) => v,
        None => {
            let (files, truncated) = walk_index(root);
            Index { files, truncated }
        }
    };
    crate::util::lk(&INDEX).insert(key, (mt, Instant::now(), v.clone()));
    Ok(v)
}

/// Os arquivos que o git vê (rastreados e não ignorados), ou None fora de um repositório.
fn git_index(root: &Path) -> Option<Index> {
    if !crate::git::is_repo(root) {
        return None;
    }
    let raw = crate::git::git_bytes(root, &["ls-files", "-co", "--exclude-standard", "-z"]).ok()?;
    let rels: Vec<String> = crate::review::split_nul(&raw).into_iter().filter(|r| !r.is_empty()).collect();
    let truncated = rels.len() > INDEX_MAX;
    let files = rels
        .into_iter()
        .take(INDEX_MAX)
        .map(|rel| IndexEntry { path: root.join(&rel).to_string_lossy().to_string(), rel })
        .collect();
    Some(Index { files, truncated })
}

/// Tira do cache as raízes que contêm `path`: o Regente chama isto ao criar, gravar, renomear ou apagar.
/// Mudança feita por fora segue valendo pelo TTL.
pub fn invalidate_within(path: &Path) {
    crate::util::lk(&INDEX).retain(|root, _| !path.starts_with(root));
}

/// Em largura, como a paleta antiga: pastas ocultas e as de IGNORED ficam de fora. Bool: cortou.
fn walk_index(root: &Path) -> (Vec<IndexEntry>, bool) {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), String::new())]);
    let mut dirs = 0;
    let mut cut = false;
    while let Some((dir, rel)) = queue.pop_front() {
        if out.len() >= INDEX_MAX || dirs >= INDEX_DIRS {
            cut = true;
            break;
        }
        dirs += 1;
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let path = e.path();
            // file_type não segue links: um link de pasta não causa laço
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if !name.starts_with('.') && !IGNORED.contains(&name.as_str()) {
                    queue.push_back((path, r));
                }
            } else if out.len() < INDEX_MAX {
                out.push(IndexEntry { rel: r, path: path.to_string_lossy().to_string() });
            } else {
                cut = true;
            }
        }
    }
    (out, cut)
}

pub fn read_text(path: &Path) -> AppResult<FileText> {
    if std::fs::metadata(path)?.len() > MAX_TEXT {
        return fail("too_big", "arquivo maior que 5 MB");
    }
    let bytes = std::fs::read(path)?;
    if bytes.contains(&0) {
        return fail("binary", "arquivo binário");
    }
    let text = String::from_utf8(bytes).map_err(|_| AppError::new("encoding", "arquivo não está em UTF-8"))?;
    Ok(FileText { text, mtime_ms: mtime_ms(path)? })
}

/// Writes text. If `expected_mtime` is given and the file changed on disk, refuses with `changed_on_disk`.
pub fn write_text(path: &Path, text: &str, expected_mtime: Option<i64>) -> AppResult<i64> {
    if let (Some(exp), true) = (expected_mtime, path.exists()) {
        if mtime_ms(path)? != exp {
            return fail("changed_on_disk", "o arquivo mudou no disco desde que foi aberto");
        }
    }
    // write-then-rename: a crash mid-write leaves the old file intact, never a half-written one
    let tmp = path.with_file_name(format!("{}.regente-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    if let Err(e) = std::fs::write(&tmp, text) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    mtime_ms(path)
}

pub fn create(path: &Path, is_dir: bool) -> AppResult<()> {
    if path.exists() {
        return fail("exists", "já existe um item com esse nome");
    }
    if is_dir {
        std::fs::create_dir_all(path)?
    } else {
        std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    }
    Ok(())
}

pub fn rename(from: &Path, to: &Path) -> AppResult<()> {
    let same = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    if to.exists() && !same {
        return fail("exists", "já existe um item com esse nome");
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// Caminho livre em `dir` para `nome`: o próprio nome se não existe; senão `a (cópia).ts`, `a (cópia 2).ts`…
pub fn nome_livre(dir: &Path, nome: &str) -> PathBuf {
    let direto = dir.join(nome);
    if !direto.exists() {
        return direto;
    }
    // a extensão fica no fim (pasta e arquivo sem ponto, ou `.env`, ficam inteiros)
    let (base, ext) = match nome.rfind('.') {
        Some(i) if i > 0 => (&nome[..i], &nome[i..]),
        _ => (nome, ""),
    };
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{base} (cópia){ext}") } else { format!("{base} (cópia {n}){ext}") }))
        .find(|p| !p.exists())
        .expect("sempre há um nome livre")
}

/// Copia arquivo ou pasta (recursivo) para dentro de `dir`, com nome livre (`nome_livre`). Devolve o caminho novo.
/// Links de pasta (junção do node_modules) não são seguidos: viram pasta vazia na cópia em vez de duplicar o alvo.
pub fn copy_into(src: &Path, dir: &Path) -> AppResult<PathBuf> {
    let nome = src.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| AppError::new("bad_path", "caminho sem nome"))?;
    if !src.exists() {
        return fail("not_found", format!("não achei {}", src.display()));
    }
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    if src.is_dir() && canon(dir).starts_with(canon(src)) {
        return fail("into_itself", "uma pasta não pode ser copiada para dentro dela mesma");
    }
    let dst = nome_livre(dir, &nome);
    copy_rec(src, &dst)?;
    Ok(dst)
}

fn copy_rec(src: &Path, dst: &Path) -> AppResult<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() && !crate::projeto::e_link(&meta) && !meta.file_type().is_symlink() {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)?.flatten() {
            copy_rec(&e.path(), &dst.join(e.file_name()))?;
        }
    } else if meta.is_dir() || (meta.file_type().is_symlink() && src.is_dir()) {
        std::fs::create_dir_all(dst)?;
    } else {
        std::fs::copy(src, dst)?;
    }
    Ok(())
}

/// Teto de pastas olhadas atrás de repositórios aninhados (um `regente/` dentro do workspace).
const ANINHADOS_MAX: usize = 64;

/// O texto de um link do terminal como caminho: aspas fora, `/` vira `\`, `~` é a pasta do usuário e `/d/x` (Git Bash)
/// vira `D:\x`.
pub fn link_texto(texto: &str, home: &Path) -> PathBuf {
    let t = texto.trim().trim_matches(|c| c == '"' || c == '\'' || c == '`');
    let t = t.replace('/', "\\");
    if let Some(resto) = t.strip_prefix("~\\") {
        return home.join(resto);
    }
    let b = t.as_bytes();
    if b.len() >= 3 && b[0] == b'\\' && b[1].is_ascii_alphabetic() && b[2] == b'\\' {
        return PathBuf::from(format!("{}:\\{}", (b[1] as char).to_ascii_uppercase(), &t[3..]));
    }
    PathBuf::from(t)
}

/// Repositórios (pasta com `.git`, pasta ou arquivo) logo abaixo de `base`.
fn aninhados(base: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(base) else { return vec![] };
    rd.flatten()
        .take(ANINHADOS_MAX * 4)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join(".git").exists())
        .take(ANINHADOS_MAX)
        .collect()
}

/// A pasta de topo do git que contém `dir` (a que tem o `.git`), se houver.
fn topo_git(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|p| p.join(".git").exists()).map(Path::to_path_buf)
}

/// Onde um caminho citado num painel pode estar, na ordem em que vale procurar: absoluto (só ele); senão a pasta do
/// painel, o topo do git dela, a raiz do workspace e os repositórios logo abaixo da pasta do painel e da raiz (o
/// Claude cita caminhos relativos ao repositório em que trabalha, que pode ser um aninhado no workspace).
pub fn candidatos_link(texto: &str, cwd: &Path, ws: &Path, home: &Path) -> Vec<PathBuf> {
    let p = link_texto(texto, home);
    if p.is_absolute() {
        return vec![p];
    }
    let mut bases = vec![cwd.to_path_buf()];
    if let Some(t) = topo_git(cwd) {
        bases.push(t);
    }
    bases.push(ws.to_path_buf());
    bases.extend(aninhados(cwd));
    bases.extend(aninhados(ws));
    let mut vistos = std::collections::HashSet::new();
    bases
        .into_iter()
        .map(|b| normalize(&b.join(&p)))
        .filter(|c| vistos.insert(c.to_string_lossy().to_lowercase()))
        .collect()
}

/// O primeiro candidato que existe; sem nenhum, um sufixo único no índice do workspace (`src/x.ts` acha
/// `pacote/src/x.ts` quando só há um).
pub fn resolver_link(texto: &str, cwd: &Path, ws: &Path, home: &Path) -> Option<PathBuf> {
    let cands = candidatos_link(texto, cwd, ws, home);
    if let Some(c) = cands.iter().find(|c| c.exists()) {
        return Some(c.clone());
    }
    let p = link_texto(texto, home);
    if p.is_absolute() || !ws.is_dir() {
        return None;
    }
    let alvo = p.to_string_lossy().replace('\\', "/").trim_start_matches("./").to_lowercase();
    if alvo.is_empty() {
        return None;
    }
    let idx = index(ws).ok()?;
    let mut achados = idx.files.iter().filter(|e| {
        let rel = e.rel.replace('\\', "/").to_lowercase();
        rel == alvo || rel.ends_with(&format!("/{alvo}"))
    });
    let um = achados.next()?;
    achados.next().is_none().then(|| PathBuf::from(&um.path))
}

/// Texto de um arquivo fora dos workspaces, para o editor abrir só leitura (clique num link do terminal que aponta para
/// AppData, `~/.claude`…). Mesmos limites do `read_text`; pasta e binário são recusados.
pub fn read_text_externo(path: &Path) -> AppResult<FileText> {
    if !path.is_absolute() {
        return fail("bad_path", "caminho relativo");
    }
    if !path.is_file() {
        return fail("not_found", format!("não achei {}", path.display()));
    }
    read_text(path)
}

/// Sends to the Recycle Bin.
pub fn delete(path: &Path) -> AppResult<()> {
    trash::delete(path).map_err(|e| AppError::new("trash_failed", e.to_string()))
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// Drops the Windows verbatim prefix that `canonicalize` adds (`\\?\C:\`, `\\?\UNC\srv\share`).
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
}

/// Canonical path (symlinks, junctions, 8.3 names resolved). A path that does not exist yet (create, rename target)
/// is canonicalized through its nearest existing parent.
fn canon(p: &Path) -> PathBuf {
    let mut tail = vec![];
    let mut cur = p;
    loop {
        if let Ok(c) = std::fs::canonicalize(cur) {
            return tail.iter().rev().fold(c, |acc, t: &std::ffi::OsString| acc.join(t));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return normalize(p),
        }
    }
}

fn within(roots: &[PathBuf], path: &Path, strict: bool) -> AppResult<()> {
    // canonicalize gives a verbatim `\\?\` prefix on Windows: strip it so keys compare equal
    let key = |p: &Path| canon(p).to_string_lossy().trim_start_matches(r"\\?\").to_lowercase().replace('/', "\\");
    let ok = path.is_absolute() && {
        let p = key(path);
        roots.iter().filter(|r| r.is_absolute()).any(|r| {
            let r = key(r);
            let r = r.trim_end_matches('\\');
            (!strict && p == r) || p.starts_with(&format!("{r}\\"))
        })
    };
    if ok { Ok(()) } else { fail("outside_workspace", "caminho fora dos workspaces abertos") }
}

/// Only lets the UI touch paths inside open workspaces (the root itself included).
pub fn ensure_within(roots: &[PathBuf], path: &Path) -> AppResult<()> {
    within(roots, path, false)
}

/// Like `ensure_within` but the root itself is refused. Use for delete/rename sources.
pub fn ensure_strictly_within(roots: &[PathBuf], path: &Path) -> AppResult<()> {
    within(roots, path, true)
}

pub const MAX_IMAGE: u64 = 32 * 1024 * 1024;

/// Real signature check (a file named .png that is not an image gets refused).
pub fn looks_like_image(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG")
        || head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(b"GIF8")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
        || head.starts_with(&[0, 0, 1, 0])
        || head.get(4..12).is_some_and(|b| b == b"ftypavif" || b == b"ftypavis")
        || String::from_utf8_lossy(head).to_lowercase().contains("<svg")
}

/// Marca do Git de um arquivo sob a raiz da árvore: caminho absoluto no formato da raiz recebida (a árvore compara
/// por texto) e o código de `gitflow::status_entries` (A, M, D, ?).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GitMarca {
    pub path: String,
    pub code: String,
    /// pasta nova inteira (`?? dir/`): tudo dentro dela é novo
    pub dir: bool,
}

/// Marcas do Git sob `root`. Fora de repositório: vazio. O `git status` dá caminhos relativos ao topo do repositório;
/// `--show-prefix` diz onde a raiz fica dentro dele (a raiz pode ser uma subpasta, e o topo pode vir com outro formato
/// de caminho, como nome longo × 8.3, então ele nunca é usado para montar o caminho).
pub fn git_marcas(root: &Path) -> AppResult<Vec<GitMarca>> {
    let Ok(prefixo) = crate::git::git(root, &["rev-parse", "--show-prefix"]) else { return Ok(vec![]) };
    let prefixo = prefixo.trim().to_string();
    let base = root.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
    Ok(crate::gitflow::status_marks(root)?
        .into_iter()
        .filter_map(|e| {
            let rel = e.path.strip_prefix(prefixo.as_str())?;
            let dir = rel.ends_with('/');
            let rel = rel.trim_end_matches('/');
            (!rel.is_empty()).then(|| GitMarca { path: format!("{base}\\{}", rel.replace('/', "\\")), code: e.code, dir })
        })
        .collect())
}

/// Teto do texto base da margem do Git (arquivos maiores ficam sem marcas).
const BASE_MAX: usize = 2 * 1024 * 1024;

/// Texto do arquivo no último commit (`HEAD`). None: fora do Git, não rastreado, sem commit, binário ou acima do teto.
pub fn git_base(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let nome = path.file_name()?.to_str()?;
    let bytes = crate::git::git_bytes(dir, &["show", &format!("HEAD:./{nome}")]).ok()?;
    if bytes.len() > BASE_MAX {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_signatures() {
        assert!(looks_like_image(b"\x89PNG\r\n\x1a\n...."));
        assert!(looks_like_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(looks_like_image(b"<?xml version=\"1.0\"?><svg xmlns"));
        assert!(!looks_like_image(b"isto e texto"));
    }

    #[test]
    fn link_do_terminal_acha_o_arquivo_no_repo_aninhado_no_topo_e_por_sufixo() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("REA");
        let home = d.path().join("home");
        // o Claude roda no worktree da missão e cita caminhos do repositório aninhado `regente/`
        let cwd = ws.join(".regente").join("worktrees").join("missao-3");
        std::fs::create_dir_all(&cwd).unwrap();
        let aninhado = ws.join("regente");
        std::fs::create_dir_all(aninhado.join(".git")).unwrap();
        std::fs::create_dir_all(aninhado.join("src-tauri").join("src")).unwrap();
        std::fs::write(aninhado.join("src-tauri").join("src").join("files.rs"), "x").unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude").join("CLAUDE.md"), "x").unwrap();
        std::fs::write(ws.join("README.md"), "x").unwrap();
        let r = |t: &str| resolver_link(t, &cwd, &ws, &home);
        assert_eq!(r("src-tauri/src/files.rs"), Some(aninhado.join("src-tauri").join("src").join("files.rs")));
        assert_eq!(r("README.md"), Some(ws.join("README.md")), "raiz do workspace");
        assert_eq!(r("~/.claude/CLAUDE.md"), Some(home.join(".claude").join("CLAUDE.md")));
        assert_eq!(r("nao/existe.ts"), None);
        let abs = aninhado.join("src-tauri").join("src").join("files.rs");
        assert_eq!(r(&abs.to_string_lossy()), Some(abs.clone()));
        assert_eq!(link_texto("/d/pessoal/a.ts", &home), PathBuf::from("D:\\pessoal\\a.ts"));
        assert_eq!(link_texto("\"src/a b.ts\"", &home), PathBuf::from("src\\a b.ts"));
    }

    #[test]
    fn copia_com_nome_livre_arquivo_e_pasta() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::write(r.join("a.ts"), "um").unwrap();
        let c1 = copy_into(&r.join("a.ts"), r).unwrap();
        assert_eq!(c1, r.join("a (cópia).ts"));
        let c2 = copy_into(&r.join("a.ts"), r).unwrap();
        assert_eq!(c2, r.join("a (cópia 2).ts"));
        assert_eq!(std::fs::read_to_string(&c2).unwrap(), "um");
        std::fs::create_dir_all(r.join("pasta").join("sub")).unwrap();
        std::fs::write(r.join("pasta").join("sub").join("x.md"), "x").unwrap();
        let destino = r.join("outra");
        std::fs::create_dir(&destino).unwrap();
        // nome livre no destino: vai com o mesmo nome
        assert_eq!(copy_into(&r.join("pasta"), &destino).unwrap(), destino.join("pasta"));
        assert_eq!(std::fs::read_to_string(destino.join("pasta").join("sub").join("x.md")).unwrap(), "x");
        assert_eq!(copy_into(&r.join("pasta"), &r.join("pasta").join("sub")).unwrap_err().code(), "into_itself");
        assert_eq!(nome_livre(r, ".env"), r.join(".env"));
    }

    #[test]
    fn lists_dirs_first_and_skips_ignored() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("node_modules")).unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("B.md"), "").unwrap();
        std::fs::write(d.path().join("a.ts"), "").unwrap();
        let names: Vec<_> = list_dir(d.path()).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["src", "a.ts", "B.md"]);
        let todos: Vec<_> = list_dir_opts(d.path(), true).unwrap().into_iter().map(|e| (e.name, e.is_dir)).collect();
        assert_eq!(todos, [("node_modules".into(), true), ("src".into(), true), ("a.ts".into(), false), ("B.md".into(), false)]);
    }

    #[test]
    fn index_skips_ignored_and_caches_until_root_changes() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/deep")).unwrap();
        std::fs::create_dir_all(r.join("node_modules/x")).unwrap();
        std::fs::write(r.join("src/deep/a.ts"), "").unwrap();
        std::fs::write(r.join("node_modules/x/b.js"), "").unwrap();
        let rels: Vec<_> = index(r).unwrap().files.into_iter().map(|e| e.rel).collect();
        assert_eq!(rels, ["src/deep/a.ts"]);
        // arquivo novo numa subpasta não muda a raiz: ainda vem do cache (até o Regente avisar)
        std::fs::write(r.join("src/deep/novo.ts"), "").unwrap();
        assert_eq!(index(r).unwrap().files.len(), 1);
        invalidate_within(&r.join("src/deep/novo.ts"));
        assert_eq!(index(r).unwrap().files.len(), 2);
        // arquivo novo na raiz muda a mtime (em ms): refaz a varredura
        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(r.join("topo.md"), "").unwrap();
        assert_eq!(index(r).unwrap().files.len(), 3);
        assert!(!index(r).unwrap().truncated);
    }

    #[test]
    fn git_index_respects_gitignore() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(r).status().unwrap().success());
        std::fs::write(r.join(".gitignore"), "segredo.env\n").unwrap();
        std::fs::write(r.join("a.ts"), "").unwrap();
        std::fs::write(r.join("segredo.env"), "").unwrap();
        let idx = index(r).unwrap();
        let mut rels: Vec<_> = idx.files.iter().map(|e| e.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, [".gitignore", "a.ts"]);
        assert!(!idx.truncated);
    }

    #[test]
    fn binary_and_conflict() {
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("x.bin");
        std::fs::write(&bin, [1u8, 0, 2]).unwrap();
        assert_eq!(read_text(&bin).unwrap_err().code(), "binary");
        let f = d.path().join("t.txt");
        let m = write_text(&f, "um", None).unwrap();
        assert_eq!(read_text(&f).unwrap(), FileText { text: "um".into(), mtime_ms: m });
        assert_eq!(write_text(&f, "dois", Some(m - 5000)).unwrap_err().code(), "changed_on_disk");
        write_text(&f, "dois", Some(m)).unwrap();
    }

    #[test]
    fn traversal_outside_root_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        let roots = vec![ws.clone()];
        assert!(ensure_within(&roots, &ws.join("a.txt")).is_ok());
        assert_eq!(ensure_within(&roots, &ws.join("..").join("fora.txt")).unwrap_err().code(), "outside_workspace");
        assert_eq!(ensure_within(&roots, &ws.join("novo").join("..").join("..").join("fora.txt")).unwrap_err().code(), "outside_workspace");
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("t.txt");
        write_text(&f, "novo", None).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "novo");
        assert!(!d.path().join("t.txt.regente-tmp").exists());
    }

    #[test]
    fn non_utf8_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.txt");
        std::fs::write(&p, [0x63u8, 0x61, 0x66, 0xE9]).unwrap(); // "café" in CP1252
        assert_eq!(read_text(&p).unwrap_err().code(), "encoding");
    }

    #[test]
    fn case_only_rename_is_allowed() {
        let d = tempfile::tempdir().unwrap();
        create(&d.path().join("abc"), false).unwrap();
        rename(&d.path().join("abc"), &d.path().join("ABC")).unwrap();
    }

    #[test]
    fn create_and_rename_refuse_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        create(&a, false).unwrap();
        assert_eq!(create(&a, false).unwrap_err().code(), "exists");
        let b = d.path().join("b");
        create(&b, true).unwrap();
        assert_eq!(rename(&a, &b).unwrap_err().code(), "exists");
        rename(&a, &d.path().join("c")).unwrap();
    }

    #[test]
    fn within_blocks_escape() {
        let roots = vec![PathBuf::from("D:\\proj\\site")];
        assert!(ensure_within(&roots, Path::new("d:/proj/site/src/a.ts")).is_ok());
        assert!(ensure_within(&roots, Path::new("D:\\proj\\site")).is_ok());
        assert!(ensure_within(&roots, Path::new("D:\\proj\\site\\..\\outro\\x")).is_err());
        assert!(ensure_within(&roots, Path::new("D:\\proj\\site2\\x")).is_err());
        assert!(ensure_within(&roots, Path::new("src\\a.ts")).is_err());
        assert!(ensure_within(&[PathBuf::from("rel")], Path::new("rel\\a")).is_err());
        assert!(ensure_strictly_within(&roots, Path::new("D:\\proj\\site")).is_err());
        assert!(ensure_strictly_within(&roots, Path::new("D:\\proj\\site\\a")).is_ok());
    }

    #[test]
    fn marcas_e_base_do_git() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let g = |a: &[&str]| crate::git::git(r, a).unwrap();
        g(&["init", "-q"]);
        g(&["config", "core.autocrlf", "false"]);
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(r.join("src").join("a.ts"), "um\n").unwrap();
        g(&["add", "."]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x"]);
        std::fs::write(r.join("src").join("a.ts"), "um\ndois\n").unwrap();
        std::fs::write(r.join("novo.md"), "n").unwrap();
        std::fs::create_dir_all(r.join("pasta-nova").join("funda")).unwrap();
        std::fs::write(r.join("pasta-nova").join("funda").join("x.txt"), "x").unwrap();
        let mut m = git_marcas(r).unwrap();
        m.sort_by(|a, b| a.path.cmp(&b.path));
        let base = r.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
        assert_eq!(m, vec![
            GitMarca { path: format!("{base}\\novo.md"), code: "?".into(), dir: false },
            // pasta nova inteira vem numa linha só (a árvore pinta o que está dentro)
            GitMarca { path: format!("{base}\\pasta-nova"), code: "?".into(), dir: true },
            GitMarca { path: format!("{base}\\src\\a.ts"), code: "M".into(), dir: false },
        ]);
        // a raiz numa subpasta: só o que está dentro dela, com o caminho a partir dela
        let sub = git_marcas(&r.join("src")).unwrap();
        assert_eq!(sub, vec![GitMarca { path: format!("{}\\a.ts", r.join("src").to_string_lossy()), code: "M".into(), dir: false }]);
        assert_eq!(git_base(&r.join("src").join("a.ts")).as_deref(), Some("um\n"));
        assert_eq!(git_base(&r.join("novo.md")), None, "não rastreado");
        let fora = tempfile::tempdir().unwrap();
        assert!(git_marcas(fora.path()).unwrap().is_empty(), "fora de repositório: vazio");
    }
}
