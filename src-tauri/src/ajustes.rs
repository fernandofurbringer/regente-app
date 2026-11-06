//! Feed "Ajustes automáticos": toda mudança que o Regente faz sozinho fica registrada, com o motivo e a estimativa.
//! Só os ajustes de perfil (`perfil_patch`) podem ser desfeitos: o desfazer devolve o patch anterior do workspace.

use crate::db::{Ajuste, Db};
use crate::error::{fail, AppResult};
use crate::perfil;
use crate::runtime::AppState;
use crate::util::{new_id, now_ms};
use serde_json::{json, Value};

/// Um ajuste a registrar. `antes`/`depois` guardam o estado antes e depois (para o desfazer).
pub struct Novo {
    pub workspace_id: Option<String>,
    pub mission_id: Option<String>,
    pub pane_id: Option<String>,
    /// perfil_patch | compactacao | recomeco | escalonamento | indice_codigo | testes_relacionados | limites_saida
    pub kind: &'static str,
    /// aplicado | proposto | pulado
    pub acao: &'static str,
    pub motivo: String,
    pub antes: Option<Value>,
    pub depois: Option<Value>,
    pub estimativa: Option<Value>,
    pub desfazivel: bool,
}

/// Teto de cada JSON guardado (`antes`/`depois`/`estimativa`), depois de redigido. Acima disso não se guarda.
const JSON_MAX: usize = 4096;
/// Quantos ajustes ficam no banco, e por quantos dias.
const KEEP: i64 = 1000;
const DAYS_90: i64 = 90 * 86_400_000;

/// JSON do ajuste, redigido e limitado, e se ele ainda serve para desfazer (`ok`): ausente, ou guardado íntegro.
/// Acima do teto ou redigido, fica None (ou a versão redigida, só para exibir) e não se desfaz.
fn guarded(v: Option<Value>) -> (Option<String>, bool) {
    let Some(v) = v else { return (None, true) };
    let raw = v.to_string();
    let text = crate::diag::redact(&raw);
    let ok = text == raw && text.len() <= JSON_MAX;
    ((text.len() <= JSON_MAX).then_some(text), ok)
}

/// Cabe no feed para desfazer: o estado (ausente, ou guardado íntegro e sem redação) passa pelo teto.
pub fn cabe(v: Option<&Value>) -> bool {
    guarded(v.cloned()).1
}

/// A regra que gerou o ajuste (`estimativa.regra`), se houver.
pub fn regra_de(a: &Ajuste) -> Option<String> {
    serde_json::from_str::<Value>(a.estimativa_json.as_deref()?).ok()?["regra"].as_str().map(String::from)
}

/// Ajustes de perfil: os que mudam o patch do workspace e por isso se desfazem pelo mesmo caminho.
fn e_de_perfil(kind: &str) -> bool {
    matches!(kind, "perfil_patch" | "indice_codigo" | "testes_relacionados" | "limites_saida" | "escalonamento")
}

/// Grava o ajuste, mantém o feed limitado (1000 linhas e 90 dias), avisa a interface (`ajuste-novo`) e devolve o id.
pub fn registrar(s: &AppState, n: Novo) -> AppResult<String> {
    let now = now_ms();
    let (antes_json, antes_ok) = guarded(n.antes);
    let (depois_json, depois_ok) = guarded(n.depois);
    let a = Ajuste {
        id: new_id(),
        created_at: now,
        workspace_id: n.workspace_id,
        mission_id: n.mission_id,
        pane_id: n.pane_id,
        kind: n.kind.into(),
        acao: n.acao.into(),
        motivo: crate::diag::redact(&n.motivo).chars().take(300).collect(),
        antes_json,
        depois_json,
        estimativa_json: guarded(n.estimativa).0,
        efeito_json: None,
        desfazivel: n.desfazivel && antes_ok && depois_ok,
        desfeito_at: None,
    };
    s.db.insert_ajuste(&a)?;
    s.db.prune_ajustes(KEEP, now - DAYS_90)?;
    let id = a.id.clone();
    s.emit("ajuste-novo", json!({"workspaceId": a.workspace_id, "ajuste": a}));
    Ok(id)
}

/// Remove o patch de perfil de um workspace (ajuste do usuário, desfazível). Sem patch, não faz nada.
pub fn remover_patch(s: &AppState, ws: &str) -> AppResult<()> {
    let antes = crate::util::lk(&s.settings).perfis.patches.get(ws).cloned();
    if antes.is_none() {
        return Ok(());
    }
    perfil::set_patch(s, ws, None)?;
    registrar(s, Novo {
        workspace_id: Some(ws.to_string()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "removido",
        motivo: "ajuste de perfil removido pelo usuário".into(), antes, depois: None, estimativa: None, desfazivel: true,
    })?;
    s.emit("perfis-changed", json!({"workspaceId": ws}));
    Ok(())
}

/// Ajustes mais recentes primeiro, até 200.
pub fn listar(db: &Db, workspace_id: Option<&str>, limit: i64) -> AppResult<Vec<Ajuste>> {
    db.list_ajustes(workspace_id, limit.clamp(1, 200))
}

/// Desfaz um ajuste de perfil: volta o patch do workspace ao `antes_json`. Só vale se o patch atual ainda é o que o
/// ajuste deixou (`depois_json`); senão o perfil mudou depois e o desfazer recusa.
pub fn desfazer(s: &AppState, id: &str) -> AppResult<()> {
    let a = s.db.get_ajuste(id)?;
    // todo ajuste que muda um patch de perfil volta pelo mesmo caminho (antes/depois)
    let ws = match (e_de_perfil(&a.kind), a.desfazivel, a.desfeito_at, a.workspace_id.as_deref()) {
        (true, true, None, Some(ws)) => ws.to_string(),
        _ => return fail("not_undoable", "este ajuste não pode ser desfeito"),
    };
    let lido = |t: &Option<String>| -> AppResult<Option<Value>> {
        t.as_deref().map(serde_json::from_str::<Value>).transpose().map(|v| v.filter(|v| !v.is_null())).map_err(|e| crate::error::AppError::new("bad_ajuste", format!("ajuste com estado ilegível: {e}")))
    };
    let depois = lido(&a.depois_json)?;
    let antes = lido(&a.antes_json)?;
    let editados = crate::util::lk(&s.settings).perfis.editados.clone();
    if let Some(p) = &antes {
        if !perfil::patch_valido(p, &editados) {
            return fail("bad_ajuste", "estado anterior do ajuste não é um perfil válido");
        }
    }
    // compara e grava sob um único lock: o patch não muda entre o teste e a escrita
    if !perfil::set_patch_if(s, &ws, depois.as_ref(), antes)? {
        // um ajuste mais novo do mesmo workspace ainda está aplicado: ele é que se desfaz primeiro
        let mais_novo = s.db.list_ajustes(Some(&ws), 1000)?.into_iter().find(|b| b.created_at > a.created_at && b.acao == "aplicado" && b.desfeito_at.is_none() && e_de_perfil(&b.kind));
        return match mais_novo {
            Some(b) => fail("stale", format!("Desfaça primeiro o ajuste mais recente ({})", b.motivo)),
            None => fail("stale", "o perfil mudou depois deste ajuste"),
        };
    }
    // a regra que gerou esta mudança não a repropõe por 30 dias: o piloto respeita o que o usuário desfez
    if let (Some(regra), Some(d)) = (regra_de(&a), depois.as_ref()) {
        crate::autoajuste::recusar(s, &ws, &regra, d);
    }
    s.db.set_ajuste_undone(id, now_ms())?;
    s.emit("ajuste-desfeito", json!({"workspaceId": ws, "id": id}));
    s.emit("perfis-changed", json!({"workspaceId": ws}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::AppState;
    use std::sync::Arc;

    fn state() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let s = AppState::new(d.path().into(), "C:/x/regente.exe".into(), Arc::new(|_: &str, _: Value| {})).unwrap();
        (d, s)
    }

    #[test]
    fn register_list_and_undo_restores_the_exact_previous_patch() {
        let (_d, s) = state();
        let antes = json!({"saidas": {"bashMax": 5000}});
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        perfil::set_patch(&s, &ws, Some(antes.clone())).unwrap();

        let depois = json!({"saidas": {"bashMax": 9000}});
        let id = registrar(&s, Novo {
            workspace_id: Some(ws.clone()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "aplicado",
            motivo: "saída do Bash passou de 5000 caracteres".into(), antes: Some(antes.clone()), depois: Some(depois.clone()),
            estimativa: None, desfazivel: true,
        })
        .unwrap();
        perfil::set_patch(&s, &ws, Some(depois)).unwrap();

        let feed = listar(&s.db, Some(&ws), 50).unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].id, id);

        desfazer(&s, &id).unwrap();
        assert_eq!(lk_patch(&s, &ws), Some(antes));
        assert!(s.db.get_ajuste(&id).unwrap().desfeito_at.is_some());
        // não desfaz duas vezes
        assert_eq!(desfazer(&s, &id).unwrap_err().code(), "not_undoable");
    }

    #[test]
    fn only_profile_patches_can_be_undone_and_motivo_is_redacted_and_clipped() {
        let (_d, s) = state();
        let id = registrar(&s, Novo {
            workspace_id: Some("ws".into()), mission_id: None, pane_id: None, kind: "compactacao", acao: "pulado",
            motivo: format!("token ghp_abcdefghijklmnopqrstuvwxyz0123456789 {}", "x".repeat(400)), antes: None, depois: None,
            estimativa: None, desfazivel: false,
        })
        .unwrap();
        let a = s.db.get_ajuste(&id).unwrap();
        assert!(a.motivo.chars().count() <= 300);
        assert!(!a.motivo.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        assert_eq!(desfazer(&s, &id).unwrap_err().code(), "not_undoable");
    }

    #[test]
    fn undo_refuses_a_patch_that_changed_since_the_adjustment() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let (a, b, c) = (json!({"saidas": {"bashMax": 5000}}), json!({"saidas": {"bashMax": 9000}}), json!({"saidas": {"bashMax": 7000}}));
        perfil::set_patch(&s, &ws, Some(a.clone())).unwrap();
        let id = registrar(&s, Novo {
            workspace_id: Some(ws.clone()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "aplicado",
            motivo: "teste".into(), antes: Some(a.clone()), depois: Some(b.clone()), estimativa: None, desfazivel: true,
        })
        .unwrap();
        perfil::set_patch(&s, &ws, Some(b.clone())).unwrap();
        // o usuário mexeu depois: o desfazer não pisa na mudança nova
        perfil::set_patch(&s, &ws, Some(c.clone())).unwrap();
        assert_eq!(desfazer(&s, &id).unwrap_err().code(), "stale");
        assert_eq!(lk_patch(&s, &ws), Some(c));
    }

    #[test]
    fn removing_a_patch_is_undoable_and_big_state_is_not_stored() {
        let (_d, s) = state();
        s.db.upsert_workspace("C:\\proj").unwrap();
        let ws = s.db.list_workspaces().unwrap()[0].id.clone();
        let p = json!({"saidas": {"bashMax": 5000}});
        perfil::set_patch(&s, &ws, Some(p.clone())).unwrap();
        remover_patch(&s, &ws).unwrap();
        assert_eq!(lk_patch(&s, &ws), None);
        let feed = listar(&s.db, Some(&ws), 10).unwrap();
        assert_eq!((feed[0].acao.as_str(), feed[0].desfazivel), ("removido", true));
        desfazer(&s, &feed[0].id).unwrap();
        assert_eq!(lk_patch(&s, &ws), Some(p));
        // sem patch, remover não registra nada
        perfil::set_patch(&s, &ws, None).unwrap();
        remover_patch(&s, &ws).unwrap();
        assert_eq!(listar(&s.db, Some(&ws), 10).unwrap().len(), 1, "só o da remoção");
        // estado grande demais não é guardado (e por isso não se desfaz)
        let big = json!({"x": "y".repeat(5000)});
        let id = registrar(&s, Novo {
            workspace_id: Some(ws.clone()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "aplicado",
            motivo: "grande".into(), antes: Some(big.clone()), depois: None, estimativa: None, desfazivel: true,
        })
        .unwrap();
        assert!(s.db.get_ajuste(&id).unwrap().antes_json.is_none());
        assert!(!s.db.get_ajuste(&id).unwrap().desfazivel, "antes cortado não se desfaz");
        assert_eq!(desfazer(&s, &id).unwrap_err().code(), "not_undoable");
        // redigido também não se desfaz (o estado guardado não é o real)
        let segredo = json!({"saidas": {"bashMax": 5000}, "x": "ghp_abcdefghijklmnopqrstuvwxyz0123456789"});
        let id = registrar(&s, Novo {
            workspace_id: Some(ws.clone()), mission_id: None, pane_id: None, kind: "perfil_patch", acao: "aplicado",
            motivo: "redigido".into(), antes: Some(segredo), depois: None, estimativa: None, desfazivel: true,
        })
        .unwrap();
        assert!(!s.db.get_ajuste(&id).unwrap().desfazivel);
        assert_eq!(desfazer(&s, &id).unwrap_err().code(), "not_undoable");
        assert_eq!(lk_patch(&s, &ws), None, "nada foi restaurado");
    }

    fn lk_patch(s: &AppState, ws: &str) -> Option<Value> {
        crate::util::lk(&s.settings).perfis.patches.get(ws).cloned()
    }
}
