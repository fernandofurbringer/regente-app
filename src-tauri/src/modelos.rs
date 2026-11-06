//! Download seguro e runtime do ONNX Runtime, compartilhados pela busca por significado (M1c) e pela voz local.
//! Cada arquivo vem de uma URL oficial com SHA-256 fixado: HTTPS apenas, `.part` apagado em qualquer erro.
//! A DLL é aberta travada (FILE_SHARE_READ), conferida pelo hash e carregada por caminho absoluto, com dependências
//! só de System32 e da pasta dela (nunca PATH nem diretório atual).

use crate::error::{fail, AppError, AppResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
#[cfg(windows)]
use windows_sys::Win32::{
    Storage::FileSystem::{GetDiskFreeSpaceExW, FILE_SHARE_READ},
    System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32},
};

/// Folga de disco exigida além do tamanho dos arquivos.
pub const FOLGA_DISCO: u64 = 200_000_000;
/// Teto da DLL copiada do zip (ela tem ~16 MB; o resto do zip nunca é gravado).
const TETO_DLL: u64 = 32 * 1024 * 1024;

/// Arquivo baixado: URL oficial e SHA-256 fixados (tamanho para mostrar o progresso e recusar excesso).
pub struct Baixado {
    pub nome: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// ONNX Runtime 1.28.0 (a versão que o `ort` 2.0.0-rc.13 exige): o zip oficial; dele só `lib/onnxruntime.dll` é usado.
pub const ORT_ZIP: Baixado = Baixado {
    nome: "onnxruntime-win-x64-1.28.0.zip",
    url: "https://github.com/microsoft/onnxruntime/releases/download/v1.28.0/onnxruntime-win-x64-1.28.0.zip",
    sha256: "abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc",
    bytes: 78_796_801,
};
const ORT_DLL_NO_ZIP: &str = "onnxruntime-win-x64-1.28.0/lib/onnxruntime.dll";
const ORT_DLL_SHA256: &str = "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257";
const ORT_DLL_BYTES: u64 = 15_809_848;

/// Flags de `LoadLibraryExW`: só o diretório da DLL (suas dependências) e System32. Sem `LOAD_WITH_ALTERED_SEARCH_PATH`:
/// ele reabre a busca pelo PATH e permite sequestro de DLL (um `onnxruntime_providers_*.dll` planta no diretório atual).
#[cfg(windows)]
pub const FLAGS_CARGA_DLL: u32 = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;

/// Uma descarga do ONNX Runtime por vez (a busca e a voz local usam o mesmo zip).
static DLL_EM_DOWNLOAD: AtomicBool = AtomicBool::new(false);
/// A DLL conferida, aberta com FILE_SHARE_READ (ninguém grava nem apaga) até o fim do processo.
static DLL_TRAVADA: OnceLock<File> = OnceLock::new();
/// O ONNX Runtime já foi carregado neste processo. Só o sucesso fica guardado: uma falha tenta de novo.
pub static ORT_OK: AtomicBool = AtomicBool::new(false);

/// Reivindica uma flag de operação (uma por vez); a flag é soltada no drop, também se o future for cancelado.
pub struct Ocupado(&'static AtomicBool);

impl Ocupado {
    pub fn tomar(flag: &'static AtomicBool) -> AppResult<Ocupado> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Ocupado(flag))
            .map_err(|_| AppError::new("ocupado", "há uma instalação ou remoção em andamento"))
    }
}

impl Drop for Ocupado {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Progresso da instalação, enviado pelo Channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "evento", rename_all = "camelCase")]
pub enum Progresso {
    Arquivo { nome: String, bytes: u64, total: u64 },
    Pronto,
}

// ---------- arquivos e integridade

pub fn pasta(data: &Path) -> PathBuf {
    data.join("models")
}

pub fn dll(data: &Path) -> PathBuf {
    pasta(data).join("onnxruntime.dll")
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_de(p: &Path) -> io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(hex(&h.finalize()))
}

/// O arquivo existe e tem o SHA-256 fixado (sem diferença de maiúsculas).
pub fn confere(p: &Path, sha: &str) -> bool {
    sha256_de(p).is_ok_and(|h| h.eq_ignore_ascii_case(sha))
}

/// Checagem barata para a tela: o arquivo existe com o tamanho fixado. O hash completo é conferido ao carregar.
pub fn presente(p: &Path, bytes: u64) -> bool {
    fs::metadata(p).is_ok_and(|m| m.len() == bytes)
}

/// Apaga o `.part` de um download ou extração que não chegou ao fim (erro, cancelamento, excesso de tamanho).
/// Depois de um rename o caminho não existe mais, então o drop não faz nada.
pub struct Parcial(pub PathBuf);

impl Drop for Parcial {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn baixar_erro(e: impl std::fmt::Display) -> AppError {
    AppError::new("download", e.to_string())
}

/// Baixa um arquivo para `<destino>.part`, confere o SHA-256 e só então renomeia. Qualquer erro apaga o parcial.
/// `cancelar` é a flag da operação dona (checada a cada pedaço).
pub async fn baixar(b: &Baixado, destino: &Path, cancelar: &AtomicBool, on: &impl Fn(Progresso)) -> AppResult<()> {
    let mut part = destino.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let _limpa = Parcial(part.clone());
    // sem timeout, uma conexão parada deixaria a instalação (e o "ocupado") presa até reiniciar; só HTTPS
    let cliente = reqwest::Client::builder().https_only(true).connect_timeout(Duration::from_secs(20)).read_timeout(Duration::from_secs(60)).build().map_err(baixar_erro)?;
    let mut resp = cliente.get(b.url).send().await.map_err(baixar_erro)?;
    if !resp.status().is_success() {
        return fail("download", format!("{}: HTTP {}", b.nome, resp.status()));
    }
    let mut f = File::create(&part)?;
    let mut h = Sha256::new();
    let mut feito = 0u64;
    while let Some(pedaco) = resp.chunk().await.map_err(baixar_erro)? {
        if cancelar.load(Ordering::SeqCst) {
            return fail("cancelado", "instalação cancelada");
        }
        feito += pedaco.len() as u64;
        if feito > b.bytes {
            return fail("download", format!("{} maior que o tamanho fixado", b.nome));
        }
        f.write_all(&pedaco)?;
        h.update(&pedaco);
        on(Progresso::Arquivo { nome: b.nome.into(), bytes: feito, total: b.bytes });
    }
    drop(f);
    if !hex(&h.finalize()).eq_ignore_ascii_case(b.sha256) {
        return fail("hash_invalido", format!("{} não bate o SHA-256 fixado", b.nome));
    }
    fs::rename(&part, destino)?;
    Ok(())
}

/// Extrai só `lib/onnxruntime.dll` do zip oficial (nome exato, no máximo `TETO_DLL` bytes).
fn extrair_dll(zip_path: &Path, destino: &Path) -> AppResult<()> {
    let mut z = zip::ZipArchive::new(File::open(zip_path)?).map_err(|e| AppError::new("zip", e.to_string()))?;
    let entrada = z.by_name(ORT_DLL_NO_ZIP).map_err(|e| AppError::new("zip", e.to_string()))?;
    let mut out = File::create(destino)?;
    io::copy(&mut io::Read::take(entrada, TETO_DLL), &mut out)?;
    Ok(())
}

/// Bytes livres no volume de `p` (None = não deu para saber; a instalação segue).
#[cfg(windows)]
pub fn espaco_livre(p: &Path) -> Option<u64> {
    let caminho: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut livre = 0u64;
    // SAFETY: `caminho` é UTF-16 terminado em nulo e vive até a chamada; o ponteiro de saída é válido.
    let ok = unsafe { GetDiskFreeSpaceExW(caminho.as_ptr(), &mut livre, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(livre)
}

#[cfg(not(windows))]
pub fn espaco_livre(_p: &Path) -> Option<u64> {
    None
}

/// Abre a DLL com FILE_SHARE_READ: enquanto o handle existir, ninguém grava nem apaga o arquivo.
#[cfg(windows)]
pub fn abrir_travado(p: &Path) -> io::Result<File> {
    fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(p)
}

#[cfg(not(windows))]
pub fn abrir_travado(p: &Path) -> io::Result<File> {
    File::open(p)
}

/// Abre e confere a DLL pelo handle travado e o guarda no processo. O hash é do mesmo arquivo que será carregado.
fn travar_dll(p: &Path) -> AppResult<()> {
    if DLL_TRAVADA.get().is_some() {
        return Ok(());
    }
    let mut f = match abrir_travado(p) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return fail("modelo_ausente", "onnxruntime.dll ausente"),
        Err(e) => return Err(e.into()),
    };
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    if !hex(&h.finalize()).eq_ignore_ascii_case(ORT_DLL_SHA256) {
        tracing::warn!("onnxruntime.dll não bate o SHA-256 fixado; não carregado");
        return fail("hash_invalido", "onnxruntime.dll não bate o SHA-256 fixado");
    }
    let _ = DLL_TRAVADA.set(f);
    Ok(())
}

/// Pré-carrega a DLL pelo caminho absoluto. As dependências vêm só de System32 e da pasta da DLL.
#[cfg(windows)]
pub fn carregar_dll(p: &Path) -> AppResult<()> {
    let caminho: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: `caminho` é UTF-16 terminado em nulo e vive até a chamada; sem handle de arquivo.
    let modulo = unsafe { LoadLibraryExW(caminho.as_ptr(), std::ptr::null_mut(), FLAGS_CARGA_DLL) };
    if modulo.is_null() {
        let cod = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        // ERROR_MOD_NOT_FOUND (126): falta uma dependência da DLL, quase sempre o VC++ runtime
        if cod == 126 {
            return fail("vc_runtime", "Instale o Microsoft Visual C++ Redistributable (x64) e tente de novo.");
        }
        return fail("ort", format!("onnxruntime.dll não carregou (erro {cod})"));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn carregar_dll(_p: &Path) -> AppResult<()> {
    Ok(())
}

/// Garante a DLL do ONNX Runtime em `<data>/models/onnxruntime.dll` (baixa o zip oficial e extrai só a DLL, se faltar).
pub async fn garantir_dll(data: &Path, cancelar: &AtomicBool, on: &impl Fn(Progresso)) -> AppResult<()> {
    fs::create_dir_all(pasta(data))?;
    if confere(&dll(data), ORT_DLL_SHA256) {
        return Ok(());
    }
    // busca e voz gravam o mesmo zip e o mesmo .part: uma descarga por vez
    let _o = Ocupado::tomar(&DLL_EM_DOWNLOAD).or_else(|_| fail("ocupado", "o ONNX Runtime já está sendo baixado; tente de novo"))?;
    let zip = pasta(data).join(ORT_ZIP.nome);
    baixar(&ORT_ZIP, &zip, cancelar, on).await?;
    let tmp = pasta(data).join("onnxruntime.dll.part");
    let _limpa = Parcial(tmp.clone());
    extrair_dll(&zip, &tmp)?;
    let _ = fs::remove_file(&zip);
    if !confere(&tmp, ORT_DLL_SHA256) {
        return fail("hash_invalido", "onnxruntime.dll não bate o SHA-256 fixado");
    }
    fs::rename(&tmp, dll(data))?;
    Ok(())
}

/// Trava, confere e carrega a DLL e inicia o `ort` uma vez por processo (só o sucesso fica guardado).
pub fn iniciar_runtime(data: &Path) -> AppResult<()> {
    let p = dll(data);
    if !p.is_absolute() {
        return fail("modelo_ausente", "diretório de dados sem caminho absoluto");
    }
    travar_dll(&p)?;
    if !ORT_OK.load(Ordering::SeqCst) {
        carregar_dll(&p)?;
        let _ = ort::init_from(&p).map_err(|e| AppError::new("ort", e.to_string()))?.commit();
        ORT_OK.store(true, Ordering::SeqCst);
    }
    Ok(())
}

/// A DLL está no lugar com o tamanho fixado (checagem barata para a tela).
pub fn dll_presente(data: &Path) -> bool {
    presente(&dll(data), ORT_DLL_BYTES)
}
