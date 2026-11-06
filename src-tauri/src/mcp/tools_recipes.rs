use super::{cap_perm, caller_row, check_doc, check_texts, finish, note, require_top, RegenteMcp};
use crate::error::{fail, AppError, AppResult};
use crate::providers::Permission;
use crate::recipes::{self, RecipeDef};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
pub struct RecipeGetArgs {
    /// Id da receita (ver recipe_list).
    pub id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RecipeSaveArgs {
    /// A receita completa (ver recipe_get para o formato).
    pub recipe: RecipeDef,
}

#[tool_router(router = recipes_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Receitas disponíveis: originais e do Regente.
    #[tool(description = "Receitas disponíveis: originais e do Regente")]
    async fn recipe_list(&self, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "recipe_list", &parts);
        finish(Ok(json!({"recipes": recipes::list(&self.state.data)})))
    }

    /// Uma receita completa.
    #[tool(description = "Uma receita completa (modo, squad, piloto, slots e respostas de intake)")]
    async fn recipe_get(&self, Parameters(a): Parameters<RecipeGetArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "recipe_get", &parts);
        finish(recipes::get(&self.state.data, &a.id).map(|r| json!(r)).ok_or_else(|| AppError::new("unknown_recipe", format!("receita desconhecida: {}", a.id))))
    }

    /// Cria uma receita do Regente. Só cria: um id já existente (de qualquer fonte) é recusado; alterar é pela UI.
    #[tool(description = "Cria uma receita do Regente. Só cria: um id já existente é recusado (alterar é pela UI)")]
    async fn recipe_save(&self, Parameters(a): Parameters<RecipeSaveArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "recipe_save", &parts);
        let data = self.state.data.clone();
        let mut r = a.recipe;
        let res = (|| -> AppResult<Value> {
            let caller = caller_row(&self.state, &parts)?;
            require_top(&caller)?;
            // a slot's permission is capped to the caller's, as every MCP-opened pane is
            for slot in r.slots.iter_mut() {
                if let Some(p) = slot.get("permission").and_then(Value::as_str).map(str::to_string) {
                    let requested: Permission = serde_json::from_value(json!(p)).map_err(|_| AppError::new("bad_slot", "permissão do slot: plan, pend ou auto"))?;
                    slot["permission"] = json!(cap_perm(&caller, Some(requested))?.as_str());
                }
            }
            if recipes::get(&data, &r.id).is_some() {
                return fail("exists", "já existe uma receita com este id (o MCP só cria)");
            }
            if r.slots.len() > 8 {
                return fail("too_big", "a receita tem no máximo 8 slots");
            }
            let mut texts: Vec<&str> = vec![r.name.as_str(), r.description.as_str()];
            texts.extend(r.style.as_deref());
            for (q, ans) in &r.intake {
                texts.push(q);
                texts.push(ans);
            }
            check_texts(&texts)?;
            check_doc(&r)?;
            recipes::save(&data, &r)?;
            Ok(json!({"ok": true, "id": r.id}))
        })();
        finish(res)
    }
}
