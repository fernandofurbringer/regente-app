//! Port do helper.rs do supertone-inc/supertonic (MIT). Ver THIRD_PARTY.md.
//! WAV PCM 16-bit mono e o latente inicial (ruído normal mascarado), como no helper.rs oficial.

/// WAV PCM 16-bit mono. Amostras fora de [-1, 1] são cortadas.
pub fn wav_pcm16(amostras: &[f32], taxa: u32) -> Vec<u8> {
    let dados = (amostras.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + dados as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + dados).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&taxa.to_le_bytes());
    w.extend_from_slice(&(taxa * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&dados.to_le_bytes());
    for &a in amostras {
        w.extend_from_slice(&((a.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    w
}

/// Normal padrão por Box-Muller sobre o `fastrand` (sem `rand`/`rand_distr`).
pub struct Ruido(fastrand::Rng);

impl Ruido {
    pub fn novo() -> Self {
        Ruido(fastrand::Rng::new())
    }
    pub fn com_semente(s: u64) -> Self {
        Ruido(fastrand::Rng::with_seed(s))
    }
    pub fn normal(&mut self) -> f32 {
        let u1 = self.0.f32().max(f32::MIN_POSITIVE);
        let u2 = self.0.f32();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

/// Latente inicial `[1, latent_dim*compress, len]` e máscara `[1, 1, len]` para uma duração em segundos.
/// Com um só texto por vez, o latente cobre a duração inteira e a máscara é toda 1.
pub fn latente_ruidoso(duracao: f32, taxa: i32, base_chunk: i32, compress: i32, latent_dim: i32, r: &mut Ruido) -> (Vec<f32>, Vec<f32>, [usize; 3]) {
    let wav_len = (duracao * taxa as f32) as usize;
    let chunk = (base_chunk * compress) as usize;
    let len = wav_len.div_ceil(chunk);
    let dim = (latent_dim * compress) as usize;
    let lat: Vec<f32> = (0..dim * len).map(|_| r.normal()).collect();
    (lat, vec![1.0; len], [1, dim, len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_tem_cabecalho_riff_e_tamanho_certo() {
        let w = wav_pcm16(&[0.0, 1.0, -1.0, 2.0], 44_100);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 44_100);
        assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 16);
        assert_eq!(w.len(), 44 + 4 * 2);
        assert_eq!(i16::from_le_bytes(w[46..48].try_into().unwrap()), 32767);
        assert_eq!(i16::from_le_bytes(w[50..52].try_into().unwrap()), 32767); // 2.0 fica em 1.0
    }

    #[test]
    fn ruido_tem_media_zero_e_variancia_um() {
        let mut r = Ruido::com_semente(42);
        let v: Vec<f32> = (0..20_000).map(|_| r.normal()).collect();
        let media = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - media).powi(2)).sum::<f32>() / v.len() as f32;
        assert!(media.abs() < 0.05, "{media}");
        assert!((var - 1.0).abs() < 0.08, "{var}");
    }

    #[test]
    fn latente_tem_forma_e_mascara_da_duracao() {
        let (lat, mask, forma) = latente_ruidoso(1.0, 44_100, 512, 6, 24, &mut Ruido::com_semente(1));
        let chunk = 512 * 6;
        let len = (44_100 + chunk - 1) / chunk;
        assert_eq!(forma, [1, 24 * 6, len]);
        assert_eq!(lat.len(), 24 * 6 * len);
        assert_eq!(mask.len(), len);
        assert!(mask.iter().all(|&m| m == 1.0));
    }
}
