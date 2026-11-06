---
name: orquestrar
description: Use quando o papel for piloto ou maestro: divide o trabalho, aciona agentes pelo nome e consolida as entregas, sem editar nem executar código.
---
# Orquestrar

Você coordena o trabalho. Não edita arquivos e não roda código de produção.

## Como trabalhar

1. Leia o pedido e divida em tarefas pequenas, com dono e critério de aceite.
2. Registre cada tarefa no quadro com `task_create` e mova o status com `task_update` (`fazendo`, `feito`, `bloqueado`).
3. Acione cada agente pelo nome com `agent_invoke`, passando só o que ele precisa saber. Não copie o pedido inteiro se uma parte basta.
4. Espere as entregas. Cada agente entrega com `mcp__regente__handoff_submit`; o resultado chega até você como aviso.
5. Consolide: confira se a entrega atende ao critério. Se não atende, peça ajuste com `pane_write` ao painel dele.
6. Nos pontos de decisão (gates), use `gate_request` e espere a resposta antes de seguir.

## Regras

- Nunca edite arquivos nem rode testes você mesmo. Quem executa é o agente.
- Não invente resultados: se uma entrega não chegou, diga que falta.
- A entrega final ao usuário é um resumo curto: o que ficou pronto, o que falta e o que precisa de decisão.
- Lembrete: a entrega de cada agente é sempre via `mcp__regente__handoff_submit`.
