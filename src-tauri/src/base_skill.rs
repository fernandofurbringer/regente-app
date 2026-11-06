//! Skill do Regente: as skills de papel (orquestrar, executar, revisar, pesquisar, planejar, testar) copiadas para a pasta
//! de skills do Claude, para valerem também fora dos painéis do Regente. Só mexe nas pastas que ele mesmo instalou
//! (marcadas com `.regente-skill`); pasta de skill do usuário com o mesmo nome fica como está. Remover manda para a Lixeira.
use crate::error::AppResult;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// As skills de papel (os papéis de `catalog::role_skill`).
pub const SKILLS: [&str; 6] = ["orquestrar", "executar", "revisar", "pesquisar", "planejar", "testar"];
const MARKER: &str = ".regente-skill";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BaseSkillStatus {
    /// Pasta de skills do Claude onde ficam as cópias (mostrada antes de qualquer escrita).
    pub target: String,
    /// Skills instaladas pelo Regente.
    pub installed: Vec<String>,
    /// Skills que ainda não estão lá.
    pub missing: Vec<String>,
    /// Já existe uma pasta com esse nome que o Regente não instalou: não é sobrescrita.
    pub conflicts: Vec<String>,
}

pub fn target_root(home: &Path) -> PathBuf {
    home.join(".claude").join("skills")
}

/// Fonte de uma skill: a do Regente (editada pelo usuário) ou a original semeada. Nunca a cópia no CLI.
fn source_of(data: &Path, name: &str) -> Option<PathBuf> {
    [data.join("skills").join(name), data.join("originals").join("skills").join(name)].into_iter().find(|p| p.join("SKILL.md").is_file())
}

fn owned(dst: &Path) -> bool {
    dst.join(MARKER).is_file()
}

pub fn status(home: &Path) -> BaseSkillStatus {
    let root = target_root(home);
    let (mut installed, mut missing, mut conflicts) = (vec![], vec![], vec![]);
    for name in SKILLS {
        let dst = root.join(name);
        if !dst.exists() {
            missing.push(name.to_string());
        } else if owned(&dst) {
            installed.push(name.to_string());
        } else {
            conflicts.push(name.to_string());
        }
    }
    BaseSkillStatus { target: root.to_string_lossy().into(), installed, missing, conflicts }
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    /// Skills copiadas (novas ou atualizadas).
    pub written: Vec<String>,
    /// Skills sem fonte (nem no Regente nem nas originais): não foram instaladas.
    pub no_source: Vec<String>,
}

/// Copia as skills de papel para a pasta do Claude (instala ou atualiza as do Regente). Pastas de terceiros com o mesmo nome ficam.
pub fn install(data: &Path, home: &Path) -> AppResult<InstallReport> {
    let root = target_root(home);
    let mut rep = InstallReport::default();
    for name in SKILLS {
        let Some(src) = source_of(data, name) else {
            rep.no_source.push(name.to_string());
            continue;
        };
        let dst = root.join(name);
        if dst.exists() && !owned(&dst) {
            continue;
        }
        // copia para uma pasta temporária, com a marca, e só então troca: interrompida, nunca sobra pasta de skill sem marca
        let tmp = root.join(format!(".regente-tmp-{name}"));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        crate::catalog::copy_dir(&src, &tmp)?;
        std::fs::write(tmp.join(MARKER), "Instalada pelo Regente. Remova pelo Regente (Configurações → Catálogo).")?;
        if dst.exists() {
            std::fs::remove_dir_all(&dst)?;
        }
        std::fs::rename(&tmp, &dst)?;
        rep.written.push(name.to_string());
    }
    Ok(rep)
}

/// Remove só as pastas instaladas pelo Regente, com `remover` (a Lixeira na UI). Devolve o que saiu.
pub fn remove_with(home: &Path, mut remover: impl FnMut(&Path) -> AppResult<()>) -> AppResult<Vec<String>> {
    let root = target_root(home);
    let mut gone = vec![];
    for name in SKILLS {
        let dst = root.join(name);
        if dst.exists() && owned(&dst) {
            remover(&dst)?;
            gone.push(name.to_string());
        }
    }
    Ok(gone)
}

pub fn remove(home: &Path) -> AppResult<Vec<String>> {
    remove_with(home, |p| trash::delete(p).map_err(|e| crate::error::AppError::new("trash", e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(data: &Path) {
        for name in SKILLS {
            let d = data.join("originals").join("skills").join(name);
            std::fs::create_dir_all(d.join("refs")).unwrap();
            std::fs::write(d.join("SKILL.md"), format!("---\nname: {name}\n---\ncorpo {name}")).unwrap();
            std::fs::write(d.join("refs").join("x.md"), "ref").unwrap();
        }
    }

    #[test]
    fn install_copies_with_marker_and_status_reports_it() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        assert_eq!(status(&home).missing.len(), 6);
        assert_eq!(install(&data, &home).unwrap().written.len(), 6);
        let s = status(&home);
        assert_eq!(s.installed.len(), 6);
        assert!(target_root(&home).join("executar").join("refs").join("x.md").is_file());
        assert!(s.target.ends_with("skills"));
    }

    #[test]
    fn a_users_own_skill_with_the_same_name_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        let mine = target_root(&home).join("revisar");
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::write(mine.join("SKILL.md"), "minha").unwrap();
        let s = status(&home);
        assert_eq!(s.conflicts, ["revisar"]);
        assert_eq!(install(&data, &home).unwrap().written.len(), 5);
        assert_eq!(std::fs::read_to_string(mine.join("SKILL.md")).unwrap(), "minha");
        assert!(remove_with(&home, |p| std::fs::remove_dir_all(p).map_err(Into::into)).unwrap().len() == 5, "removes only its own");
        assert!(mine.join("SKILL.md").is_file());
    }

    #[test]
    fn remove_goes_through_the_remover_and_only_touches_owned_folders() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        install(&data, &home).unwrap();
        let mut seen = vec![];
        let gone = remove_with(&home, |p| {
            seen.push(p.file_name().unwrap().to_string_lossy().to_string());
            std::fs::remove_dir_all(p).map_err(Into::into)
        })
        .unwrap();
        assert_eq!(gone.len(), 6);
        assert_eq!(seen.len(), 6);
        assert!(status(&home).installed.is_empty());
    }

    #[test]
    fn every_installed_folder_carries_the_marker_and_no_temp_folder_is_left() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        install(&data, &home).unwrap();
        let root = target_root(&home);
        for entry in std::fs::read_dir(&root).unwrap() {
            let p = entry.unwrap().path();
            assert!(owned(&p), "pasta sem marca: {}", p.display());
        }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 6);
    }

    #[test]
    fn reinstall_updates_the_owned_folders() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        install(&data, &home).unwrap();
        std::fs::write(data.join("originals").join("skills").join("executar").join("SKILL.md"), "versao nova").unwrap();
        let rep = install(&data, &home).unwrap();
        assert_eq!(rep.written.len(), 6, "atualização é permitida");
        assert_eq!(std::fs::read_to_string(target_root(&home).join("executar").join("SKILL.md")).unwrap(), "versao nova");
    }

    #[test]
    fn a_skill_without_source_is_reported_not_silently_skipped() {
        let d = tempfile::tempdir().unwrap();
        let (data, home) = (d.path().join("data"), d.path().join("home"));
        seed(&data);
        std::fs::remove_dir_all(data.join("originals").join("skills").join("planejar")).unwrap();
        let rep = install(&data, &home).unwrap();
        assert_eq!(rep.no_source, ["planejar"]);
        assert_eq!(rep.written.len(), 5);
    }
}
