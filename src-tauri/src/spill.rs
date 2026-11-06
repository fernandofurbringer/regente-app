//! Histórico de terminal que saiu do buffer em memória (`ring`): vai para `panes/<id>/scrollback.log`.
//!
//! Teto total de 20 MB: cada arquivo tem 10 MB; quando o ativo enche, ele vira `scrollback.1.log` (substitui o
//! anterior) e começa outro. O diretório do painel é apagado junto com o painel, então o arquivo some com ele.
//!
//! A escrita acontece numa thread própria (`SpillWriter`), alimentada por canal limitado: o leitor do PTY nunca
//! espera disco. Se o canal lota, os bytes são descartados e contados em `dropped`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;

pub const SPILL_FILE: &str = "scrollback.log";
pub const ROTATED_FILE: &str = "scrollback.1.log";
/// Teto de cada arquivo; com os dois, o total fica em 20 MB.
pub const SPILL_FILE_MAX: u64 = 10 * 1024 * 1024;
/// Blocos pendentes entre o leitor do PTY e a thread de escrita (cada bloco é um chunk do ring).
const QUEUE_CHUNKS: usize = 256;

pub struct Spill {
    dir: PathBuf,
    max: u64,
    written: u64,
    /// Rotation failed once (file locked): logged once, then every rotation truncates the active file.
    rotate_warned: AtomicBool,
}

impl Spill {
    pub fn new(dir: PathBuf) -> Self {
        Self::with_limit(dir, SPILL_FILE_MAX)
    }

    pub fn with_limit(dir: PathBuf, max: u64) -> Self {
        // a pasta é criada aqui (no spawn), não no append: uma escrita atrasada depois de fechar o painel não recria a pasta
        let _ = std::fs::create_dir_all(&dir);
        let written = std::fs::metadata(dir.join(SPILL_FILE)).map(|m| m.len()).unwrap_or(0);
        Self { dir, max, written, rotate_warned: AtomicBool::new(false) }
    }

    /// Acrescenta bytes que saíram do buffer, rotacionando antes se o ativo passaria do teto.
    pub fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if self.written > 0 && self.written + bytes.len() as u64 > self.max {
            self.rotate()?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(SPILL_FILE))?;
        f.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Ativo vira `.1` (substituindo o antigo). Se o `.1` está preso (outro processo com ele aberto, p.ex. o
    /// antivírus no Windows), trunca o ativo em vez de falhar a cada append; avisa uma vez só.
    fn rotate(&mut self) -> std::io::Result<()> {
        let active = self.dir.join(SPILL_FILE);
        match std::fs::rename(&active, self.dir.join(ROTATED_FILE)) {
            Ok(()) => {}
            Err(e) => {
                if !self.rotate_warned.swap(true, Ordering::Relaxed) {
                    tracing::warn!("histórico em disco: não rotacionou ({e}); truncando o arquivo ativo");
                }
                std::fs::File::create(&active)?;
            }
        }
        self.written = 0;
        Ok(())
    }
}

/// Dono da thread de escrita. `push` nunca bloqueia: canal cheio ou thread morta = bytes contados em `dropped`.
pub struct SpillWriter {
    tx: SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
}

impl SpillWriter {
    pub fn spawn(spill: Spill) -> Self {
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_CHUNKS);
        let mut spill = spill;
        // se a thread não sobe, `rx` cai com a closure e todo `push` vira `dropped`
        let _ = thread::Builder::new().name("pty-spill".into()).spawn(move || {
            for chunk in rx {
                if let Err(e) = spill.append(&chunk) {
                    tracing::warn!("histórico em disco indisponível: {e}");
                }
            }
        });
        Self { tx, dropped: Arc::new(AtomicU64::new(0)) }
    }

    pub fn push(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = self.tx.try_send(bytes) {
            if self.dropped.fetch_add(1, Ordering::Relaxed) == 0 {
                tracing::warn!("histórico em disco: fila cheia, descartando saída antiga");
            }
        }
    }

    /// Blocos descartados por canal cheio ou thread morta.
    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Últimos `max` bytes do histórico em disco, na ordem certa (rotacionado, depois o ativo). Lê só a cauda de
/// cada arquivo (seek até o fim), então não carrega o histórico inteiro. `None` se não há arquivo.
pub fn tail(dir: &Path, max: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut any = false;
    // do mais novo para o mais velho; cada pedaço mais velho entra na frente
    for name in [SPILL_FILE, ROTATED_FILE] {
        if out.len() >= max {
            break;
        }
        let Ok(mut f) = std::fs::File::open(dir.join(name)) else { continue };
        any = true;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let take = len.min((max - out.len()) as u64);
        if f.seek(SeekFrom::Start(len - take)).is_err() {
            continue;
        }
        let mut chunk = vec![0u8; take as usize];
        if f.read_exact(&mut chunk).is_err() {
            continue;
        }
        chunk.extend(out);
        out = chunk;
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_limit_and_keeps_both_files_in_order() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Spill::with_limit(d.path().to_path_buf(), 10);
        s.append(b"aaaaaa").unwrap(); // 6
        s.append(b"bbbbbb").unwrap(); // 6+6 > 10: o ativo vira .1
        assert_eq!(std::fs::read(d.path().join(ROTATED_FILE)).unwrap(), b"aaaaaa");
        assert_eq!(std::fs::read(d.path().join(SPILL_FILE)).unwrap(), b"bbbbbb");
        assert_eq!(tail(d.path(), usize::MAX).unwrap(), b"aaaaaabbbbbb");
    }

    #[test]
    fn a_second_rotation_replaces_the_old_rotated_file_so_disk_use_stays_capped() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Spill::with_limit(d.path().to_path_buf(), 10);
        for chunk in [b"aaaaaa", b"bbbbbb", b"cccccc"] {
            s.append(chunk).unwrap();
        }
        assert_eq!(std::fs::read(d.path().join(ROTATED_FILE)).unwrap(), b"bbbbbb", "o .1 antigo ('aaaaaa') foi substituído");
        assert_eq!(tail(d.path(), usize::MAX).unwrap(), b"bbbbbbcccccc");
    }

    #[test]
    fn reopening_continues_the_active_file_and_no_file_means_no_history() {
        let d = tempfile::tempdir().unwrap();
        assert!(tail(d.path(), usize::MAX).is_none());
        Spill::with_limit(d.path().to_path_buf(), 100).append(b"xy").unwrap();
        let mut again = Spill::with_limit(d.path().to_path_buf(), 100);
        again.append(b"z").unwrap();
        assert_eq!(tail(d.path(), usize::MAX).unwrap(), b"xyz");
    }

    #[test]
    fn tail_reads_only_the_newest_bytes_across_both_files() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Spill::with_limit(d.path().to_path_buf(), 10);
        s.append(b"aaaaaa").unwrap();
        s.append(b"bbbbbb").unwrap(); // .1 = aaaaaa, ativo = bbbbbb
        assert_eq!(tail(d.path(), 8).unwrap(), b"aabbbbbb", "pega o fim do .1 e o ativo inteiro");
        assert_eq!(tail(d.path(), 3).unwrap(), b"bbb", "pede só o necessário do ativo");
    }

    #[test]
    fn a_locked_rotated_file_truncates_the_active_one_instead_of_failing_every_append() {
        let d = tempfile::tempdir().unwrap();
        // um diretório no lugar do .1 impede o rename (simula arquivo preso)
        std::fs::create_dir(d.path().join(ROTATED_FILE)).unwrap();
        let mut s = Spill::with_limit(d.path().to_path_buf(), 10);
        s.append(b"aaaaaa").unwrap();
        s.append(b"bbbbbb").unwrap(); // rename falha: trunca o ativo e grava
        s.append(b"cccccc").unwrap(); // continua funcionando nas rotações seguintes
        assert_eq!(std::fs::read(d.path().join(SPILL_FILE)).unwrap(), b"cccccc");
        assert!(s.rotate_warned.load(Ordering::Relaxed));
    }

    #[test]
    fn writer_thread_persists_pushed_chunks_and_counts_nothing_when_healthy() {
        let d = tempfile::tempdir().unwrap();
        let w = SpillWriter::spawn(Spill::with_limit(d.path().to_path_buf(), 100));
        w.push(b"ola ".to_vec());
        w.push(b"mundo".to_vec());
        w.push(Vec::new());
        // a thread é assíncrona: espera o arquivo aparecer com o conteúdo completo
        for _ in 0..200 {
            if tail(d.path(), usize::MAX).is_some_and(|h| h == b"ola mundo") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(tail(d.path(), usize::MAX).unwrap(), b"ola mundo");
        assert_eq!(w.dropped(), 0);
    }
}
