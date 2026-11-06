use super::{caller_row, finish, note, RegenteMcp};
use crate::error::AppResult;
use crate::planexec::{self, Plano};
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, JsonSchema)]
pub struct ReplyArgs {
    /// Id da tarefa no plano (ex.: T3).
    pub task: String,
    /// Resposta objetiva para o executor (uma ou duas frases).
    pub texto: String,
}

// Só o planejador da missão chama estas ferramentas (checado em `planexec`). Papel curto (executor) não as vê.
#[tool_router(router = plan_router, vis = "pub(crate)")]
impl RegenteMcp {
    /// Entrega o plano da missão ao usuário (só o planejador). Abre o gate "Aprovar plano"; devolve versão e tarefas.
    #[tool(description = "Entrega o plano da missão ao usuário (só o planejador). Abre o gate Aprovar/Ajustar e devolve a versão e o número de tarefas")]
    async fn plan_submit(&self, Parameters(p): Parameters<Plano>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "plan_submit", &parts);
        finish((|| -> AppResult<Value> {
            let me = caller_row(&self.state, &parts)?;
            planexec::enviar(&self.state, &me, p)
        })())
    }

    /// Responde a pergunta de um executor da missão (só o planejador). A resposta entra no painel do executor.
    #[tool(description = "Responde a pergunta de um executor da missão (só o planejador). A resposta vai para o painel do executor")]
    async fn plan_reply(&self, Parameters(a): Parameters<ReplyArgs>, Extension(parts): Extension<http::request::Parts>) -> CallToolResult {
        note(&self.state, "plan_reply", &parts);
        finish((|| -> AppResult<Value> {
            let me = caller_row(&self.state, &parts)?;
            planexec::responder(&self.state, &me, &a.task, &a.texto)?;
            Ok(json!({"ok": true}))
        })())
    }
}

#[cfg(test)]
mod tests {
    use super::super::tools_for_role;

    #[test]
    fn executor_never_sees_plan_tools_and_keeps_handoff_submit() {
        let vistas = tools_for_role(Some("executor")).unwrap();
        assert!(!vistas.contains(&"plan_submit") && !vistas.contains(&"plan_reply"));
        assert!(vistas.contains(&"handoff_submit"));
        assert!(tools_for_role(Some("planner")).is_none(), "o planejador é papel longo: vê tudo");
    }
}
