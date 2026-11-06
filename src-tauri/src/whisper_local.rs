//! Whisper local embutido: o `whisper-server.exe` oficial do whisper.cpp (b5454, MIT) e um de dois modelos (MIT):
//! `preciso` (large-v3-turbo q5_0) ou `leve` (small q5_1), baixados por ação do usuário para
//! `<dados>/models/whisper.cpp/`. Cada arquivo tem SHA-256 fixado. Antes de cada início do servidor os binários e o
//! modelo em uso são reabertos com FILE_SHARE_READ (ninguém troca nem apaga enquanto ele roda) e conferidos pelo hash
//! do próprio handle.

use crate::error::{fail, AppError, AppResult};
use crate::modelos::{abrir_travado, baixar, confere, espaco_livre, hex, pasta, presente, Baixado, Ocupado, Parcial, Progresso, FOLGA_DISCO};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;

/// Pasta dentro de `<dados>/models/`.
pub const PASTA: &str = "whisper.cpp";
pub const SERVIDOR: &str = "whisper-server.exe";

pub const ZIP: Baixado = Baixado {
    nome: "whisper-bin-x64.zip",
    url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip",
    sha256: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb",
    bytes: 8_928_640,
};

/// Modelo `preciso` (padrão): large-v3-turbo quantizado. Costuma acertar mais em português.
pub const PRECISO: Baixado = Baixado {
    nome: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin",
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    bytes: 574_041_195,
};

/// Modelo `leve`: small quantizado (~5x mais rápido, menos memória).
pub const LEVE: Baixado = Baixado {
    nome: "ggml-small-q5_1.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small-q5_1.bin",
    sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    bytes: 190_085_487,
};

/// Ids aceitos em `VoiceConfig.whisper_modelo`, na ordem de preferência quando o escolhido não está baixado.
pub const IDS: [&str; 2] = ["preciso", "leve"];

/// O modelo de um id (None se desconhecido).
pub fn modelo(id: &str) -> Option<&'static Baixado> {
    match id {
        "preciso" => Some(&PRECISO),
        "leve" => Some(&LEVE),
        _ => None,
    }
}

fn modelo_ou_erro(id: &str) -> AppResult<&'static Baixado> {
    modelo(id).ok_or_else(|| AppError::new("bad_whisper_modelo", "modelo do Whisper local: preciso ou leve"))
}

/// Arquivos tirados de `Release/` no zip: nome, SHA-256 e bytes. Só estes; os outros executáveis nunca são gravados.
pub const BINARIOS: &[(&str, &str, u64)] = &[
    ("whisper-server.exe", "a97456e747f28e7bb09c1dac35f40d94404c5ecffa74aaaaa7a3753ea6543121", 726_016),
    ("whisper.dll", "034396d75bb5720c1851674ff64a96192b58f6927b68d334ebd254ce6cad9674", 1_370_112),
    ("ggml.dll", "31fe18616d872af9b6134b47a6dff8072ae4ecbe5df0273f4f8dc36766c9db8e", 62_464),
    ("ggml-base.dll", "bede0d6b2d387236ecd2b5dd676d51c8984b5b6e0ae461a12174e9c01345c5f0", 693_248),
    ("ggml-cpu-alderlake.dll", "d8f5c56ea490394eeae8b271650f2647aecd039a542d6df9e0140726e62a4603", 1_009_664),
    ("ggml-cpu-cannonlake.dll", "74127b40abd4f595d9db6af0fa313a02e18ff42fc228bfa05281e7c788dd4507", 1_049_600),
    ("ggml-cpu-cascadelake.dll", "4b11c427b844eeac87529af9a8d94b35aad2024666beb2a35e59fe3077e4b81e", 1_061_376),
    ("ggml-cpu-haswell.dll", "1306b1014c8118d27234d8ddc0e66ced1f8c8476bf85071595fd182ea9b7263b", 1_010_688),
    ("ggml-cpu-icelake.dll", "2b4dc4c66e7d8fe2807283816e9cf3b7f7d0556f59df34069871e37eab8aed40", 1_061_376),
    ("ggml-cpu-sandybridge.dll", "04edbdebc01a1d421062a960881a4f34d520d99693999518243f332406444c6e", 1_026_560),
    ("ggml-cpu-skylakex.dll", "1774714cf93621406ff35344b54715619a83242ae3e331e44d98a3b35ce801be", 1_049_600),
    ("ggml-cpu-sse42.dll", "f060318772c4c2dbac312bbfff6ea700b85dbc502bb38585a7186d06d549301a", 862_720),
    ("ggml-cpu-x64.dll", "4b4b05caba40d4309b5d710ef74c59ba190516b8489f27bdb1ba11ae33c6e9d3", 866_816),
];

/// Instalação ou remoção em curso (o servidor embutido não sobe enquanto isso).
pub fn ocupado() -> bool {
    INSTALANDO.load(Ordering::SeqCst)
}

/// Uma instalação ou remoção por vez.
static INSTALANDO: AtomicBool = AtomicBool::new(false);
/// Cancelamento da instalação em curso: checado a cada pedaço baixado.
static CANCELAR: AtomicBool = AtomicBool::new(false);

pub fn pasta_whisper(data: &Path) -> PathBuf {
    pasta(data).join(PASTA)
}

/// Caminhos absolutos do servidor e do modelo `id`.
pub fn caminhos(data: &Path, id: &str) -> AppResult<(PathBuf, PathBuf)> {
    let m = modelo_ou_erro(id)?;
    let dir = pasta_whisper(data);
    Ok((dir.join(SERVIDOR), dir.join(m.nome)))
}

/// Os 13 binários presentes com o tamanho fixado (checagem barata; o hash é conferido em `travar`).
fn binarios_ok(dir: &Path) -> bool {
    BINARIOS.iter().all(|(n, _, b)| presente(&dir.join(n), *b))
}

/// O arquivo do modelo `id` está lá com o tamanho fixado.
pub fn modelo_instalado(data: &Path, id: &str) -> bool {
    modelo(id).is_some_and(|m| presente(&pasta_whisper(data).join(m.nome), m.bytes))
}

/// O modelo a usar: o preferido se estiver instalado; senão o outro; None se faltar binário ou não houver modelo.
pub fn escolher(data: &Path, preferido: &str) -> Option<&'static str> {
    if !binarios_ok(&pasta_whisper(data)) {
        return None;
    }
    let ordem = if preferido == "leve" { ["leve", "preciso"] } else { ["preciso", "leve"] };
    ordem.into_iter().find(|id| modelo_instalado(data, id))
}

/// Estado para a tela: por modelo, se está instalado e o tamanho; se os binários estão; se há instalação em curso.
pub fn status(data: &Path) -> Value {
    let info = |id: &str| json!({ "instalado": modelo_instalado(data, id), "bytes": modelo(id).map_or(0, |m| m.bytes) });
    json!({
        "binarios": binarios_ok(&pasta_whisper(data)),
        "bytesZip": ZIP.bytes,
        "modelos": { "preciso": info("preciso"), "leve": info("leve") },
        "instalando": ocupado(),
    })
}

/// Baixa o modelo `id` e, se faltarem, o zip (extrai só os 13 arquivos). Só roda por ação do usuário.
pub async fn instalar(data: &Path, id: &str, on: impl Fn(Progresso)) -> AppResult<()> {
    let m = modelo_ou_erro(id)?;
    let _o = Ocupado::tomar(&INSTALANDO)?;
    // um servidor rodando segura os arquivos: para antes de regravar
    crate::stt::stop_local();
    soltar_travas();
    CANCELAR.store(false, Ordering::SeqCst);
    let r = instalar_arquivos(data, id, m, &on).await;
    if r.is_err() {
        // cancelado ou falhou antes de qualquer arquivo: tira a pasta vazia (remove_dir não apaga pasta com arquivo)
        let _ = fs::remove_dir(pasta_whisper(data));
    }
    r
}

async fn instalar_arquivos(data: &Path, id: &str, m: &'static Baixado, on: &impl Fn(Progresso)) -> AppResult<()> {
    let dir = pasta_whisper(data);
    fs::create_dir_all(&dir)?;
    if let Some(livre) = espaco_livre(&pasta(data)) {
        // os binários só contam se faltarem; o modelo só se ainda não estiver lá
        let motor = if binarios_ok(&dir) { 0 } else { ZIP.bytes + BINARIOS.iter().map(|b| b.2).sum::<u64>() };
        let arquivo = if modelo_instalado(data, id) { 0 } else { m.bytes };
        let preciso = motor + arquivo + FOLGA_DISCO;
        if livre < preciso {
            return fail("espaco", format!("Espaço insuficiente no disco: são necessários {} MB livres.", preciso / 1_000_000));
        }
    }
    let d = dir.clone();
    let completo = tokio::task::spawn_blocking(move || BINARIOS.iter().all(|(n, sha, _)| confere(&d.join(n), sha)))
        .await
        .map_err(|e| AppError::new("whisper_failed", e.to_string()))?;
    if !completo {
        let zip = dir.join(ZIP.nome);
        // o zip sai em qualquer caso (sucesso, erro ou cancelamento): só os 13 arquivos ficam
        let _sem_zip = Parcial(zip.clone());
        baixar(&ZIP, &zip, &CANCELAR, on).await?;
        let (z, d) = (zip.clone(), dir.clone());
        tokio::task::spawn_blocking(move || extrair(&z, &d)).await.map_err(|e| AppError::new("whisper_failed", e.to_string()))??;
    }
    let arquivo = dir.join(m.nome);
    let a = arquivo.clone();
    let ok = tokio::task::spawn_blocking(move || confere(&a, m.sha256)).await.map_err(|e| AppError::new("whisper_failed", e.to_string()))?;
    if !ok {
        baixar(m, &arquivo, &CANCELAR, on).await?;
    }
    on(Progresso::Pronto);
    Ok(())
}

/// Extrai só os arquivos de `BINARIOS` (nome exato em `Release/`, no máximo o tamanho fixado + 1), cada um por um
/// `.part` conferido antes do rename. Um arquivo que não bate o hash para tudo e não deixa `.part`.
fn extrair(zip: &Path, dir: &Path) -> AppResult<()> {
    let mut z = zip::ZipArchive::new(File::open(zip)?).map_err(|e| AppError::new("zip", e.to_string()))?;
    for (nome, sha, bytes) in BINARIOS {
        let part = dir.join(format!("{nome}.part"));
        let _limpa = Parcial(part.clone());
        {
            let entrada = z.by_name(&format!("Release/{nome}")).map_err(|e| AppError::new("zip", e.to_string()))?;
            let mut out = File::create(&part)?;
            io::copy(&mut io::Read::take(entrada, bytes + 1), &mut out)?;
        }
        if !confere(&part, sha) {
            return fail("hash_invalido", format!("{nome} não bate o SHA-256 fixado"));
        }
        fs::rename(&part, dir.join(nome))?;
    }
    Ok(())
}

/// Abre o servidor, as DLLs e o modelo `id` com FILE_SHARE_READ e confere cada um pelo hash do próprio handle. Enquanto
/// os handles vivem, ninguém grava, renomeia nem apaga esses arquivos: o que roda é o que foi conferido. O outro modelo
/// (se baixado) não é aberto: o servidor só lê o que recebe em `-m`, e `.bin` não é código.
pub fn travar(data: &Path, id: &str) -> AppResult<Vec<File>> {
    let m = modelo_ou_erro(id)?;
    let dir = pasta_whisper(data);
    let mut travas = vec![];
    for (nome, sha, _) in BINARIOS.iter().copied().chain([(m.nome, m.sha256, m.bytes)]) {
        let mut f = match abrir_travado(&dir.join(nome)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return fail("whisper_missing", "Whisper local incompleto: baixe de novo em Configurações → Voz"),
            Err(e) => return Err(e.into()),
        };
        let mut h = Sha256::new();
        io::copy(&mut f, &mut h)?;
        if !hex(&h.finalize()).eq_ignore_ascii_case(sha) {
            tracing::warn!("whisper local: {nome} não bate o SHA-256 fixado; servidor não iniciado");
            return fail("hash_invalido", format!("{nome} não bate o SHA-256 fixado"));
        }
        travas.push(f);
    }
    // com os arquivos travados, nada além dos 13 binários e dos dois modelos pode estar na pasta (o servidor carrega
    // DLLs do próprio diretório)
    if let Some(n) = estranhos(&dir).into_iter().next() {
        return fail("hash_invalido", format!("arquivo estranho na pasta do Whisper local: {n}"));
    }
    Ok(travas)
}

/// Arquivos já conferidos nesta sessão (chave: pasta de dados + modelo). Ficam travados com FILE_SHARE_READ até o app
/// fechar ou até instalar/remover (`soltar_travas`): ninguém os altera nesse meio-tempo, então o hash do modelo
/// (574 MB, ~1–2 s) não se repete a cada vez que o servidor sobe, o que pesava no "liberar logo após o ditado".
static CONFERIDOS: std::sync::Mutex<Option<(String, Arc<Vec<File>>)>> = std::sync::Mutex::new(None);

/// `travar` uma vez por sessão e modelo; nas próximas, só a checagem barata de arquivos estranhos na pasta.
pub fn travar_sessao(data: &Path, id: &str) -> AppResult<Arc<Vec<File>>> {
    let chave = format!("{}|{id}", data.display());
    let mut g = crate::util::lk(&CONFERIDOS);
    if let Some((k, travas)) = g.as_ref() {
        if *k == chave {
            if let Some(n) = estranhos(&pasta_whisper(data)).into_iter().next() {
                return fail("hash_invalido", format!("arquivo estranho na pasta do Whisper local: {n}"));
            }
            return Ok(travas.clone());
        }
    }
    // outro modelo (ou outra pasta): solta as travas antigas antes de conferir de novo
    *g = None;
    let travas = Arc::new(travar(data, id)?);
    *g = Some((chave, travas.clone()));
    Ok(travas)
}

/// Solta as travas da sessão: antes de instalar ou remover arquivos (travados, eles não poderiam ser trocados).
pub fn soltar_travas() {
    *crate::util::lk(&CONFERIDOS) = None;
}

/// Entradas da pasta que não são um dos 13 binários nem um dos dois modelos (diretórios também contam).
fn estranhos(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != PRECISO.nome && n != LEVE.nome && !BINARIOS.iter().any(|(b, _, _)| b == n))
        .collect()
}

/// Apaga o modelo `id` (o servidor é parado antes, senão o arquivo está travado). Sem nenhum modelo sobrando, apaga a
/// pasta inteira, binários também.
pub fn remover(data: &Path, id: &str) -> AppResult<()> {
    let m = modelo_ou_erro(id)?;
    let _o = Ocupado::tomar(&INSTALANDO)?;
    crate::stt::stop_local();
    soltar_travas();
    remover_arquivos(&pasta_whisper(data), m)
}

fn remover_arquivos(dir: &Path, m: &Baixado) -> AppResult<()> {
    match fs::remove_file(dir.join(m.nome)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // sem modelo, os binários não servem para nada
    if !IDS.iter().filter_map(|i| modelo(i)).any(|x| dir.join(x.nome).exists()) {
        match fs::remove_dir_all(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

// ---------- comandos (Tauri)

#[tauri::command]
pub fn whisper_local_status(s: State<'_, Arc<crate::runtime::AppState>>) -> Value {
    status(&s.data)
}

/// Baixa um modelo do Whisper local (e o motor, se faltar). Depois de instalado, o ditado passa a usar o motor local
/// com esse modelo (dá para voltar à nuvem).
#[tauri::command(async)]
pub async fn whisper_local_instalar(s: State<'_, Arc<crate::runtime::AppState>>, modelo: String, on_event: Channel<Progresso>) -> AppResult<()> {
    let data = s.data.clone();
    instalar(&data, &modelo, move |p| {
        let _ = on_event.send(p);
    })
    .await?;
    let mut v = crate::stt::load(&data);
    v.engine = "local".into();
    v.whisper_modelo = modelo;
    crate::stt::save(&data, &v)?;
    Ok(())
}

#[tauri::command]
pub fn whisper_local_cancelar() {
    CANCELAR.store(true, Ordering::SeqCst);
}

/// Apaga um modelo. Se o ditado usava o Whisper local (sem caminho manual) e não sobrou nenhum modelo, volta para a
/// nuvem (Groq).
#[tauri::command(async)]
pub fn whisper_local_remover(s: State<'_, Arc<crate::runtime::AppState>>, modelo: String) -> AppResult<()> {
    remover(&s.data, &modelo)?;
    let mut v = crate::stt::load(&s.data);
    if v.engine == "local" && v.whisper_server.as_deref().is_none_or(|p| p.trim().is_empty()) && escolher(&s.data, &v.whisper_modelo).is_none() {
        v.engine = "groq".into();
        crate::stt::save(&s.data, &v)?;
    }
    Ok(())
}

/// Só em testes: pasta com os 13 binários e os modelos `ids`, vazios mas com o tamanho fixado (nada é baixado).
#[cfg(test)]
pub fn instalar_falso(data: &Path, ids: &[&str]) {
    let dir = pasta_whisper(data);
    fs::create_dir_all(&dir).unwrap();
    for (n, _, b) in BINARIOS {
        File::create(dir.join(n)).unwrap().set_len(*b).unwrap();
    }
    for id in ids {
        let m = modelo(id).unwrap();
        File::create(dir.join(m.nome)).unwrap().set_len(m.bytes).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn so_o_servidor_e_dlls_saem_do_zip() {
        assert_eq!(BINARIOS.len(), 13);
        for (n, sha, b) in BINARIOS {
            assert!(*n == SERVIDOR || n.ends_with(".dll"), "{n}");
            assert_eq!(sha.len(), 64);
            assert!(*b > 0);
        }
        assert!(!BINARIOS.iter().any(|(n, _, _)| *n == "SDL2.dll"));
    }

    /// Zip com o nome certo e o conteúdo errado: recusa, sem deixar `.part` nem o arquivo final; os outros executáveis
    /// do zip nunca são gravados.
    #[test]
    fn extrair_recusa_conteudo_adulterado() {
        let d = tempfile::tempdir().unwrap();
        let zip_path = d.path().join("w.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("Release/whisper-server.exe", o).unwrap();
            z.write_all(b"nao e o servidor").unwrap();
            z.start_file("Release/main.exe", o).unwrap();
            z.write_all(b"x").unwrap();
            z.finish().unwrap();
        }
        let dir = d.path().join("out");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(extrair(&zip_path, &dir).unwrap_err().code(), "hash_invalido");
        assert!(fs::read_dir(&dir).unwrap().next().is_none(), "nada gravado");
    }

    #[test]
    fn modelos_fixados_sao_dois_e_distintos() {
        assert_eq!(IDS, ["preciso", "leve"]);
        assert_ne!(PRECISO.nome, LEVE.nome);
        assert_ne!(PRECISO.sha256, LEVE.sha256);
        for m in [&PRECISO, &LEVE] {
            assert_eq!(m.sha256.len(), 64);
            assert!(m.url.starts_with("https://huggingface.co/") && m.url.ends_with(m.nome));
        }
        assert_eq!(LEVE.bytes, 190_085_487);
        assert!(modelo("preciso").is_some() && modelo("leve").is_some() && modelo("enorme").is_none());
    }

    #[test]
    fn travar_recusa_ausente_adulterado_e_modelo_desconhecido() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(travar(d.path(), "preciso").unwrap_err().code(), "whisper_missing");
        assert_eq!(travar(d.path(), "enorme").unwrap_err().code(), "bad_whisper_modelo");
        let dir = pasta_whisper(d.path());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(SERVIDOR), b"trocado").unwrap();
        assert_eq!(travar(d.path(), "leve").unwrap_err().code(), "hash_invalido");
    }

    #[test]
    fn travar_sessao_recusa_ausente_e_soltar_limpa() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(travar_sessao(d.path(), "leve").unwrap_err().code(), "whisper_missing");
        soltar_travas();
        assert!(crate::util::lk(&CONFERIDOS).is_none());
    }

    #[test]
    fn estranhos_aceita_os_dois_modelos_e_recusa_outro() {
        let d = tempfile::tempdir().unwrap();
        File::create(d.path().join(SERVIDOR)).unwrap();
        File::create(d.path().join(PRECISO.nome)).unwrap();
        File::create(d.path().join(LEVE.nome)).unwrap();
        assert!(estranhos(d.path()).is_empty(), "os dois modelos coexistem");
        File::create(d.path().join("ggml-tiny.bin")).unwrap();
        File::create(d.path().join("evil.dll")).unwrap();
        fs::create_dir(d.path().join("sub")).unwrap();
        let mut e = estranhos(d.path());
        e.sort();
        assert_eq!(e, ["evil.dll", "ggml-tiny.bin", "sub"]);
    }

    #[test]
    fn modelo_instalado_confere_o_tamanho_de_cada_um() {
        let d = tempfile::tempdir().unwrap();
        assert!(!modelo_instalado(d.path(), "preciso"));
        instalar_falso(d.path(), &["leve"]);
        assert!(modelo_instalado(d.path(), "leve"));
        assert!(!modelo_instalado(d.path(), "preciso"), "o outro não foi baixado");
        assert!(!modelo_instalado(d.path(), "enorme"));
        File::create(pasta_whisper(d.path()).join(LEVE.nome)).unwrap().set_len(10).unwrap();
        assert!(!modelo_instalado(d.path(), "leve"), "tamanho errado");
    }

    #[test]
    fn escolher_usa_o_preferido_cai_no_outro_e_none_sem_nada() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(escolher(d.path(), "preciso"), None, "nada baixado");
        instalar_falso(d.path(), &["leve"]);
        assert_eq!(escolher(d.path(), "preciso"), Some("leve"), "cai no outro");
        instalar_falso(d.path(), &["preciso", "leve"]);
        assert_eq!(escolher(d.path(), "preciso"), Some("preciso"));
        assert_eq!(escolher(d.path(), "leve"), Some("leve"));
        assert_eq!(escolher(d.path(), "lixo"), Some("preciso"), "id desconhecido vale o padrão");
        // sem um binário, nenhum modelo basta
        fs::remove_file(pasta_whisper(d.path()).join("ggml.dll")).unwrap();
        assert_eq!(escolher(d.path(), "preciso"), None);
    }

    #[test]
    fn remover_um_modelo_mantem_os_binarios_e_o_ultimo_apaga_a_pasta() {
        let d = tempfile::tempdir().unwrap();
        instalar_falso(d.path(), &["preciso", "leve"]);
        let dir = pasta_whisper(d.path());
        remover_arquivos(&dir, &PRECISO).unwrap();
        assert!(!dir.join(PRECISO.nome).exists());
        assert!(dir.join(LEVE.nome).exists() && dir.join(SERVIDOR).exists(), "sobrou o leve e o motor");
        remover_arquivos(&dir, &LEVE).unwrap();
        assert!(!dir.exists(), "sem modelo nenhum, a pasta inteira sai");
        remover_arquivos(&dir, &LEVE).unwrap();
    }

    #[test]
    fn status_traz_cada_modelo_com_instalado_e_tamanho() {
        let d = tempfile::tempdir().unwrap();
        let v = status(d.path());
        assert_eq!(v["binarios"], false);
        assert_eq!(v["modelos"]["preciso"]["instalado"], false);
        assert_eq!(v["modelos"]["leve"]["bytes"], 190_085_487u64);
        instalar_falso(d.path(), &["leve"]);
        let v = status(d.path());
        assert_eq!(v["binarios"], true);
        assert_eq!(v["modelos"]["leve"]["instalado"], true);
        assert_eq!(v["modelos"]["preciso"]["instalado"], false);
        assert_eq!(v["modelos"]["preciso"]["bytes"], 574_041_195u64);
        assert_eq!(v["bytesZip"], 8_928_640u64);
    }
}
