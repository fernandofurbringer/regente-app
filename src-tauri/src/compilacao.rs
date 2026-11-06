//! Teto de compilação compartilhado por todas as panes (e pela verificação/pré-aquecimento): um jobserver do GNU make
//! com `fichas(..)` vagas, que todo `cargo` herda por `CARGO_MAKEFLAGS`. Sem isso, cada worktree compila com todos os
//! núcleos ao mesmo tempo e a RAM estoura (paginação, disco a 100%). Cada `cargo` ainda tem uma vaga implícita: o pico
//! fica em `fichas + cargos simultâneos` processos `rustc`, não em `núcleos × cargos`. Com o Regente fechado, o `cargo`
//! só avisa que não achou o jobserver e compila como sempre.
use std::sync::OnceLock;

const GIB: f64 = (1u64 << 30) as f64;

/// Uma vaga a cada 4 GiB de RAM (um `rustc` pesado passa de 1 GiB, o linker mais), entre 1 e o número de núcleos.
pub fn fichas(ram_bytes: u64, nucleos: usize) -> usize {
    ((ram_bytes as f64 / GIB).round() as usize / 4).clamp(1, nucleos.max(1))
}

/// Variáveis para o ambiente de um processo que pode compilar. Vazio fora do Windows ou se o semáforo não foi criado.
pub fn ambiente() -> &'static [(String, String)] {
    static AMB: OnceLock<Vec<(String, String)>> = OnceLock::new();
    AMB.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let nucleos = std::thread::available_parallelism().map_or(1, |n| n.get());
        let n = fichas(sys.total_memory(), nucleos);
        criar(n).map(|nome| vec![("CARGO_MAKEFLAGS".into(), makeflags(&nome))]).unwrap_or_default()
    })
}

/// O mesmo formato que o crate `jobserver` passa aos filhos; o `cargo` lê o nome depois de `--jobserver-auth=`.
fn makeflags(nome: &str) -> String {
    format!("-j --jobserver-fds={nome} --jobserver-auth={nome}")
}

/// Cria o semáforo nomeado com `n` vagas. O handle nunca é fechado: é ele que mantém o nome vivo enquanto o Regente roda.
#[cfg(windows)]
fn criar(n: usize) -> Option<String> {
    use windows_sys::Win32::System::Threading::CreateSemaphoreW;
    let nome = format!("regente_jobserver_{}", std::process::id());
    let largo: Vec<u16> = nome.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `largo` é UTF-16 terminado em zero e vive até o fim da chamada; sem atributos de segurança (padrão).
    let h = unsafe { CreateSemaphoreW(std::ptr::null(), n as i32, n as i32, largo.as_ptr()) };
    if h.is_null() {
        tracing::warn!("jobserver de compilação não criado: {}", std::io::Error::last_os_error());
        return None;
    }
    tracing::info!("jobserver de compilação: {n} vagas ({nome})");
    Some(nome)
}

#[cfg(not(windows))]
fn criar(_: usize) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fichas_seguem_a_ram_dentro_dos_nucleos() {
        let gib = |g: u64| g << 30;
        assert_eq!(fichas(gib(16) - (300 << 20), 16), 4, "16 GB reportados como 15,7 GiB");
        assert_eq!(fichas(gib(8), 8), 2);
        assert_eq!(fichas(gib(4), 8), 1);
        assert_eq!(fichas(gib(2), 8), 1, "nunca zero");
        assert_eq!(fichas(gib(64), 8), 8, "nunca acima dos núcleos");
    }

    #[test]
    fn makeflags_entrega_o_nome_como_o_cargo_le() {
        let v = makeflags("regente_jobserver_7");
        // o mesmo recorte do crate `jobserver` (`find_jobserver_auth`)
        let lido = v.rsplit_once("--jobserver-auth=").map(|(_, s)| s).and_then(|s| s.split(' ').next());
        assert_eq!(lido, Some("regente_jobserver_7"));
    }

    #[cfg(windows)]
    #[test]
    fn o_semaforo_abre_pelo_nome_como_o_cargo_abre() {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenSemaphoreW, SEMAPHORE_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE};
        let amb = ambiente();
        let (k, v) = amb.first().expect("CARGO_MAKEFLAGS");
        assert_eq!(k, "CARGO_MAKEFLAGS");
        let nome = v.rsplit_once("--jobserver-auth=").unwrap().1;
        let largo: Vec<u16> = nome.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: nome UTF-16 terminado em zero; o handle aberto é fechado logo depois.
        let h = unsafe { OpenSemaphoreW(SYNCHRONIZATION_SYNCHRONIZE | SEMAPHORE_MODIFY_STATE, 0, largo.as_ptr()) };
        assert!(!h.is_null(), "o cargo não acharia o semáforo {nome}");
        unsafe { CloseHandle(h) };
        assert!(std::ptr::eq(amb, ambiente()), "um semáforo só por processo");
    }
}
