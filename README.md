# Regente

Regente é um cockpit para Windows para trabalhar com CLIs de IA para código (Claude Code, Codex, Gemini). Ele abre vários
painéis de terminal lado a lado, mede o custo de cada um, organiza o trabalho em missões e acrescenta voz, mídia e controle pelo celular.
É um app desktop (Tauri 2, backend em Rust, interface em React/TypeScript) e roda tudo na sua máquina: não existe servidor do Regente.

## O que o Regente faz

- Você abre uma pasta como workspace e põe vários agentes de IA para trabalhar em painéis lado a lado.
- Uma missão agrupa painéis e, se você quiser, um worktree git próprio. Você revisa, commita e integra sem sair do app.
- Ele mostra quanto cada painel gasta (tokens e dólares) e quanto contexto está em uso, e sugere o que cortar para gastar menos.
- Um perfil de trabalho (Econômico, Equilibrado, Máxima precisão) decide modelo, esforço e limites de saída. Você troca em uma tela.
- Tudo fica na sua máquina. As chaves ficam no Cofre (DPAPI do Windows) e nada é enviado ao Regente.

## Funcionalidades

Os grupos seguem o menu de Configurações. Cada item diz como chegar nele: tecla, paleta (Ctrl+Shift+P) ou menu.

### IAs e agentes

- **Suas IAs**: detecta Claude Code, Codex e Gemini, instala e faz login pelo próprio painel. Rotas alternativas (OpenRouter, DeepSeek, Ollama).
  Configurações → Suas IAs. Abrir um painel: `+ PAINEL` ou "Novo painel de IA" (Ctrl+Shift+N, repete o último).
- **Painéis**: terminais de IA, shells, navegador, imagem e vídeo. Ctrl+Shift+T abre um terminal. Ctrl+1 a Ctrl+9 focam um painel.
  Ctrl+Tab e Ctrl+Shift+Tab andam entre painéis. Ctrl+Shift+F busca em todos os painéis.
- **Missões**: `NOVA MISSÃO` ou Ctrl+Shift+M. Missão com worktree git isola os agentes. Revisar, commitar e integrar ficam na barra da missão.
- **Missão agêntica e squads**: um piloto delega tarefas a ajudantes. Squads têm maestro, fases e gates de aprovação.
  Configurações → Missão agêntica e Squads. Gate pendente: Ctrl+Shift+G.
- **Perfil de trabalho**: escolhe modelo, esforço e limites por tipo de tarefa. O chip do perfil aparece no cabeçalho de cada painel.
  Configurações → Perfil de trabalho (ou "Configurações: Perfil de trabalho" na paleta). Detalhes em "Economia de tokens".
- **Projeto**: `.regente/projeto.json` guarda comandos de build e de teste, glossário e perfil do projeto. Configurações → Projeto.
  A pasta `.regente/` fica fora do `git status`; para dividir o arquivo com a equipe, `git add -f .regente/projeto.json` uma vez.
- **Execuções**: comandos de shell que o agente roda aparecem num painel "Execuções". Comandos longos viram painel próprio.
  Paleta: "Mostrar execuções do painel ativo" e "Fechar execuções concluídas". Configurações → Execuções.
- **Compositor**: mensagem com várias linhas e menções (`@arquivo`, `@diff`, `@erro`). Ctrl+Shift+Enter abre no painel de IA ativo.
  Enviar ao agente: "Enviar seleção ao agente…" na paleta, ou o aviso "Erro detectado" no cabeçalho de um terminal com erro.
- **Checkpoints e linha do tempo**: o Regente guarda o estado da missão quando um agente termina a vez. "Checkpoint agora" e
  "Linha do tempo de checkpoints" na paleta, ou ⋯ da missão → Linha do tempo. Voltar a um checkpoint guarda o estado atual antes.
- **Catálogo, receitas e stacks**: agentes e skills, receitas de missão e stacks de painéis. Configurações → Catálogo e Receitas.
  Paleta: "Salvar painéis abertos como stack" e "Abrir stack: …".
- **Prompts**: textos salvos e variáveis. Paleta: "Prompts salvos" e "prompt: …". Configurações → Prompts.
- **Memória do workspace**: fatos, decisões e riscos que os agentes recebem em cada painel. Configurações → Memória.
  Revisar sugestões: paleta "Revisar sugestões de memória".
- **Agendamentos**: rodar um agente em horário fixo. Configurações → Agendamentos.

### Voz e mídia

- **Voz**: ditado no painel ativo com Ctrl+Space (tecla configurável). Motores: Groq, compatível com OpenAI ou whisper.cpp local.
  Configurações → Voz. O ditado global e os atalhos globais ficam desligados por padrão.
- **Ditado local**: Configurações → Voz → Baixar Whisper local (~583 MB). Roda no computador, sem chave.
- **IA auxiliar**: um modelo de texto (Anthropic ou compatível com OpenAI) para as sugestões: mensagem de commit, "Me sugere"
  do briefing, instruções do projeto, resumo de ausência e ditado em inglês no modo código. Configurações → IA auxiliar.
- **Voz local (Supertonic 3)**: Configurações → Voz → Baixar voz local (~400 MB). Vira a voz da Mídia e da ferramenta MCP de áudio, sem chave. ElevenLabs continua opcional (e é o único para efeitos sonoros).
- **Mídia**: imagem (Gemini, GPT Image), vídeo (Replicate) e áudio (voz local ou ElevenLabs), com limite de gasto. Configurações → Mídia.
- **Captura de tela**: tela, região, gravação e galeria. Paleta "Galeria de capturas" ou ⋯ → Capturas de tela.
  Os atalhos globais de captura vêm desligados. Configurações → Captura de tela.
- **Arquivos**: soltar arquivos do Explorer sobre um terminal cola os caminhos entre aspas (sem Enter).

### Custos e limites

- **Custos e orçamento**: gasto por painel, missão e dia, com tokens sem e com cache. A barra do topo mostra "Hoje" e o orçamento diário.
  Configurações → Custos e orçamento (a seção antes se chamava "Seu Regente (uso)").
- **Preços**: tabela de preços por modelo, usada no cálculo de custo. Configurações → Preços.
- **Limites de uso**: limites das contas Claude e Codex (janelas de 5 h e 7 dias) com histórico. Botão de limites na barra do topo, ou paleta "Limites de uso".
- **Monitor de recursos**: memória e CPU do Regente, da interface e de cada painel (com os processos filhos). Paleta "Monitor de recursos",
  ou o indicador de RAM no topo. Dali dá para hibernar os painéis ociosos.
- **Hibernação**: um painel ocioso libera memória e retoma com `--resume`. Paleta "Hibernar painel ativo".
  Configurações → Hibernação.

### Dispositivos e segurança

- **Celular**: PWA para acompanhar e controlar painéis pela rede local. Desligado por padrão. Configurações → Celular.
- **Cofre**: chaves de API protegidas pelo DPAPI do Windows. Configurações → Cofre.
- **Atualizações**: canal assinado, com verificação da assinatura antes de instalar. Configurações → Atualizações.

### Aplicativo

- **Aparência e geral**: tema, fonte, terminal, concorrência e permissões das ferramentas do Regente. Configurações → Aparência e geral.
- **Notificações**: aviso quando um agente termina ou pergunta. Configurações → Notificações.
- **Atalhos**: veja a lista abaixo. Configurações → Atalhos.
- **Logs e diagnóstico**: logs e zip de diagnóstico, sem segredos. Configurações → Logs e diagnóstico.
- **Cards de início**: paleta "Mostrar cards de início".

## Atalhos

Atalhos da janela (não são globais do sistema). Dentro do terminal, Ctrl+letra vai para o programa; por isso o Regente usa Ctrl+Shift,
Ctrl+Alt, Ctrl+dígito e Ctrl+Tab. Fora do terminal, todos valem. Todos podem ser trocados em Configurações → Atalhos.

| Ação | Atalho |
|------|--------|
| Paleta de comandos | Ctrl+Shift+P (extra: Ctrl+K, fora do terminal) |
| Abrir arquivo | Ctrl+Shift+O (extra: Ctrl+P, fora do terminal) |
| Novo painel de IA (como o último) | Ctrl+Shift+N |
| Novo terminal | Ctrl+Shift+T |
| Nova missão | Ctrl+Shift+M |
| Ir para painel aguardando resposta | Ctrl+Shift+A |
| Abrir gate pendente | Ctrl+Shift+G |
| Compositor do painel de IA | Ctrl+Shift+Enter |
| Copiar conversa do painel ativo | Ctrl+Shift+C (com seleção, vale o copiar nativo) |
| Copiar última resposta do agente | Ctrl+Shift+Y |
| Buscar em todos os painéis | Ctrl+Shift+F |
| Próximo / painel anterior | Ctrl+Tab / Ctrl+Shift+Tab |
| Focar painel 1 a 9 | Ctrl+1 … Ctrl+9 |
| Ditar no painel ativo | Ctrl+Space |

## Economia de tokens

Tudo abaixo é decidido pelo perfil de trabalho (Configurações → Perfil de trabalho). O perfil vale quando o painel abre.
Mudar o padrão afeta só painéis novos, porque trocar a configuração no meio de uma sessão perde o cache.

- **Perfis**: Econômico (menos tokens, recomeça em vez de compactar), Equilibrado (o padrão) e Máxima precisão (modelos maiores e mais contexto).
  Cada perfil traz modelo e esforço por tipo de tarefa, limites de saída e de contexto. "Editar valores (avançado)" ajusta um perfil em JSON;
  "Restaurar padrão" volta ao original. Cada missão pode ter o seu perfil no assistente "Nova missão" (campo "Perfil desta missão").
- **Chip do perfil**: aparece no cabeçalho do painel e na missão. Um `*` indica ajuste feito. O tooltip mostra a origem e o que foi aplicado.
- **Ajustes automáticos com desfazer**: cada mudança automática fica no feed "Ajustes automáticos" (Configurações → Perfil de trabalho).
  Mostra quando, o quê, por quê e o efeito. Um ajuste de perfil se desfaz com "Desfazer", que restaura o valor exato anterior.
- **Piloto automático de eficiência**: na aba "Ajustes automáticos", escolha o modo: **Aplicar sozinho**, **Só sugerir** ou **Desligado**.
  Ele roda a cada 10 min e ao abrir Custos e orçamento. Ele nunca rebaixa `planejar` nem `revisar`; sugerir subir `implementar`
  é sempre só proposta (botões Aplicar e Ignorar). A única descida automática é o slot `explorar` para Haiku, quando as tarefas
  mecânicas dão certo. "Desfazer" e "Ignorar" são respeitados por 30 dias: a mesma mudança não volta nesse prazo.
  "Zerar aprendizado" apaga o que ele aprendeu com as compactações.
- **Saídas enxutas**: quando um comando de shell dá certo e a saída passa do limite do perfil, o agente recebe um resumo
  (contagens e falhas, se a suíte for reconhecida). A saída completa vai para `.regente/saidas/` do workspace.
  Falhas nunca são resumidas: o erro chega inteiro. O limite é `saidas.resumirAcima`; `BASH_MAX_OUTPUT_LENGTH` e o limite de leitura também vêm do perfil.
- **MCP enxuto**: agentes de papel curto (ajudantes, executores, revisores, testadores) só veem o conjunto básico de ferramentas do Regente.
  As outras ficam ocultas para eles, o que poupa o texto de cada ferramenta em todo turno.
- **Artefatos**: resultado grande não entra na conversa. A entrega leva caminho, formato, tamanho e resumo, e o agente lê só as linhas
  ou campos de que precisa (ferramenta `artifact_read`). Ficam em `.regente/artefatos/`.
- **Medidor de contexto**: barra fina no cabeçalho do painel: verde até 50 %, âmbar até 75 %, vermelho acima. O tooltip mostra os tokens.
  Some enquanto não há medida (início da sessão ou logo após `/compact`).
- **Compactação por custo**: o Regente compacta ou recomeça um painel só quando a conta fecha:
  o custo da compactação precisa se pagar em poucos turnos. Na fronteira de uma tarefa, papéis longos recomeçam com um brief de retomada
  (missão, quadro de tarefas, últimas entregas). Painéis do usuário nunca compactam sozinhos: só recebem a proposta, em um chip
  "Recomeçar com memória" ou "Compactar" no medidor de contexto. Toda decisão aparece no feed com a estimativa.
  A compactação ociosa nativa do Claude Code fica desligada por padrão.

## Execução rápida

- **Plano → Execução**: estilo de missão em que um planejador estuda o código e escreve um plano; você aprova; executores rápidos
  fazem cada tarefa, em paralelo dentro do limite do perfil. Assistente "Nova missão" → estilo "Plano → Execução", ou paleta
  "Nova missão Plano → Execução". O plano fica em `plano.md`. O gate "Aprovar plano" (Aprovar ou Ajustar) lista primeiro
  **"Comandos que o Regente vai rodar"**, com cada critério de aceite e a tarefa dele. Só a versão aprovada roda.
  Um critério de aceite só vira comando se for de teste ou build (`npm test`, `cargo test`, `vitest`, `tsc` e parecidos), numa linha
  e sem metacaracteres de shell (como `&`, `|`, `;`, `%`, `!`, parênteses e aspas). Critério em prosa não é executado: o executor o cumpre.
- **Verificação**: depois que um executor entrega, o Regente roda os testes relacionados às mudanças e, no fim da missão, a suíte
  de aceite completa. O chip da barra mostra `✔ testes 42 s`, `✘ falhou · 1m10` ou `rodando…`; "Cancelar rodada" para a verificação.
  Se falhar, o erro volta ao mesmo executor; na segunda falha ele sobe um degrau de modelo, se houver; na terceira a tarefa fica
  "bloqueada" e o planejador recebe o erro. Qualquer missão pode ter verificação ao terminar (⋯ da missão → Verificação…, com
  "Rodar ao terminar um agente"). Comando de `projeto.json` só roda depois que você confia nele.
- **Worktrees rápidos**: worktrees de missão reaproveitam o cache de build do Cargo e o `node_modules` do checkout principal.
  O `node_modules` entra por **hardlink, ligado por padrão**, com aviso de que os arquivos são compartilhados com o checkout principal:
  não edite nada dentro de `node_modules` no worktree. A junção (`nodeModulesJuncao`) vem **desligada** e só entra como plano B
  se você ligar. O venv compartilhado (`venvCompartilhado`) é **opt-in**. Ajuste em `.regente/projeto.json`, chave `cacheBuild`.
- **Comparar abordagens**: paleta "Comparar abordagens (mesmo pedido, variantes)…". O mesmo pedido vai para 2 a 4 variantes
  (CLI ou modelo diferente), cada uma num worktree próprio. A tela mostra arquivos, custo, tempo e verificação de cada uma.
  "Ficar com esta" pede confirmação listando o que será integrado e o que será arquivado. Mudanças não commitadas das outras
  variantes vão para um commit wip na branch delas e não entram no projeto. Só existe pela paleta.
- **Desempenho da missão**: tempo e tokens por fase (planejamento, execução, verificação, correções). Barra da missão → "Desempenho da missão".
- **Enquanto você estava fora**: depois de alguns minutos sem foco (padrão 10; Configurações → Enquanto você estava fora, 0 desliga),
  ao voltar aparece um resumo local por missão: agentes que terminaram ou erraram, perguntas, gates, entregas, arquivos e verificação.
  "Resumir com IA" é opcional. Paleta: "O que aconteceu enquanto eu estava fora".
- **Execuções em painel**: comandos longos (servidor de dev, testes em modo contínuo) viram painel de saída ao vivo e fecham sozinhos
  quando dão certo. Configurações → Execuções define os tempos (promover após 5 s; fechar após 20 s).
  **Servidores**: quando um terminal sobe um endereço local, o chip "Servidores (n)" na barra do topo lista o servidor, com
  "Abrir no navegador interno", "Abrir no navegador do sistema", "Ir ao painel" e "Parar" (manda Ctrl+C). "Parar" só existe para painel
  de terminal, e não há "Reiniciar". Porta ocupada mostra a dica "Porta N em uso por outro painel".

## Memória

- A memória é por workspace. Fatos, decisões e riscos entram no contexto de cada agente como dados, não como instruções.
  Configurações → Memória.
- **Sugestões**: o Regente sugere memória ao fim de uma entrega, de uma decisão de gate ou de uma integração. Você aceita, edita ou descarta.
  Paleta "Revisar sugestões de memória". Aceitação automática é opt-in.
- **Fixar e arquivar**: memória fixada entra sempre. "Esquecer memórias não usadas após N dias" arquiva (nunca apaga) as antigas.
- **Preferências globais**: valem em todos os workspaces; no máximo 5 entram em cada painel.
- **Instruções do projeto**: Configurações → Memória → Instruções do projeto. Lê `CLAUDE.md` e `AGENTS.md` do workspace e sugere
  uma atualização com a memória aceita. A sugestão aparece em diff; nada é gravado sem "Aplicar", e só se o arquivo não mudou desde a leitura.
- **Higiene das instruções**: Configurações → Memória → "Higiene das instruções". Mostra o custo fixo estimado por turno dos
  arquivos de instrução e avisos sobre eles. "Copiar prompt de limpeza" copia um prompt para você usar; o Regente não grava nada.
- **Busca por significado**: opt-in em Configurações → Memória ("Buscar por significado"). Ao ligar, baixa uma vez o ONNX Runtime 1.28
  (~79 MB; do zip oficial só a DLL é extraída) e o modelo multilíngue e5-small em fp32 (~470 MB). Cada arquivo é conferido por SHA-256
  antes de uso. Precisa do Microsoft Visual C++ Redistributable x64; se faltar, o Regente pede a instalação. "Cancelar" interrompe o
  download. Com a busca desligada, "Remover arquivos baixados" apaga o modelo. Sem ele, a busca usa palavras-chave.

## Segurança

- **Níveis de permissão por painel:** *Padrão do CLI* não passa argumento e vale o modo configurado no próprio CLI (no Claude
  Code pode ser perguntar ou automático); *auto* aceita edições
  automaticamente; *plan* só planeja; *danger* (Perigo) pula todas as permissões da CLI. Use *danger* só em pastas descartáveis.
- **Nada roda sem aprovação:** comandos definidos em `.regente/projeto.json` só rodam automaticamente depois que você confia neles
  (Configurações → Projeto → "Confiar nos comandos deste projeto"). A confiança é guardada pelo hash dos comandos: se eles mudarem, ela cai.
  Um repositório clonado com um `projeto.json` estranho não roda nada sozinho.
- **Ferramentas MCP do Regente:** Configurações → Aparência e geral → "Permitir as ferramentas do Regente sem perguntar" pré-autoriza
  as ferramentas do servidor embutido nos painéis do Regente. Desmarque para que cada chamada peça confirmação. Ferramentas que mudam
  estado (abrir painel, escrever, capturar tela, enviar à rede) continuam pedindo aprovação em painéis *pend* e *plan*.
- **Links e caminhos:** `.regente` e `projeto.json` não podem ser links simbólicos ou junções: o Regente não grava nem lê por eles.
  Caminhos vindos da interface ou de agentes são recusados se saírem da raiz do workspace. Pastas pesadas (`node_modules`, `target`, `dist`)
  ficam fora da leitura dos agentes.
- **Downloads:** só acontecem por ação sua. A atualização do app é verificada pela assinatura antes de instalar. O ONNX Runtime
  e o modelo da busca por significado são baixados só depois de você ligar a função, e conferidos por SHA-256.
- **Celular:** desligado por padrão. Ao ligar, o pareamento é por QR (token de uso único, 10 min) ou por código de 6 dígitos que só
  aparece quando você pede e exige a sua aprovação no computador. Texto para painéis de IA é enviado direto; para terminais, só depois
  de você confirmar no desktop. O celular não vê arquivos nem abre painel em modo *danger*. "Apenas HTTP" escuta só no loopback; fora
  dele o servidor usa HTTPS com certificado próprio. Você pode revogar qualquer aparelho.
- **Desligado por padrão:** ditado global, atalhos globais de captura, captura de tela pelo agente, servidor do celular, atualização
  automática (sem canal configurado), hibernação automática.
- **Chaves:** ficam no Cofre (DPAPI, só o seu usuário lê). Nunca vão para logs, diagnóstico ou celular.
- Nada é enviado ao Regente: não há servidor, telemetria ou conta. O áudio, o texto e as imagens só vão ao provedor que você configurou.

## Primeira execução

1. Siga o onboarding e abra uma pasta como workspace (Início → Abrir pasta, ou passe a pasta na linha de comando: `regente.exe <pasta>`).
2. Configurações → Suas IAs mostra as CLIs detectadas; instale ou faça login pelo próprio painel.
3. `+ PAINEL` abre um painel de IA, `+ TERMINAL` abre um shell, `NOVA MISSÃO` cria uma missão.
4. Os dados ficam em `%APPDATA%\Regente`. A variável `REGENTE_HOME` troca essa pasta.

## Chaves e provedores

As chaves ficam no **Cofre** (Configurações → Cofre), protegidas pelo DPAPI do Windows (só o seu usuário lê). Nunca vão para logs,
diagnóstico ou para o celular. Cada recurso usa um nome de chave:

| Recurso | Onde configurar | Chave no Cofre |
|---------|-----------------|----------------|
| IA auxiliar (Anthropic) | Configurações → IA auxiliar | `jarvis.anthropic` |
| IA auxiliar (OpenAI ou compatível) | Configurações → IA auxiliar | `jarvis.openai` |
| Voz (Groq ou compatível) | Configurações → Voz | `voice.groq` |
| Imagem Gemini | Configurações → Mídia | `media.google` |
| Imagem GPT Image | Configurações → Mídia | `media.openai` |
| Vídeo Replicate | Configurações → Mídia | `media.replicate` |
| Áudio ElevenLabs | Configurações → Mídia | `media.elevenlabs` |

Em cada tela, "Salvar chave" grava no Cofre e "Testar" faz uma leitura gratuita da conta do provedor. A conta Claude dos limites de uso usa o
login da própria CLI (`~/.claude`), só leitura. Rotas alternativas de modelo (OpenRouter, DeepSeek, Ollama) ficam em Suas IAs.

## Requisitos

- Windows 10 ou 11 (64 bits, x64) e WebView2 (já vem no Windows 11; o instalador baixa se faltar).
- Pelo menos uma CLI de IA instalada e logada: Claude Code, Codex ou Gemini.
- **Git for Windows (com Git Bash)** recomendado: missões em worktree, revisão e commit usam o git da máquina.
- **Microsoft Visual C++ Redistributable x64**: só para a busca por significado.
- Opcionais: `ffmpeg` (gravação e vídeo da captura de tela), `whisper.cpp` (transcrição local), OpenSSH no Windows e `tmux` no servidor (workspaces remotos).

## Instalação

1. Baixe `Regente_<versão>_x64-setup.exe` e execute (instalador NSIS, por usuário).
2. Abra o Regente pelo menu Iniciar. A atualização automática (opcional) é configurada em Configurações → Atualizações.

## Lacunas conhecidas

- Não verificados em hardware real: microfone (ditado), captura de tela e gravação, atalhos globais,
  workspaces remotos com servidor real, celular em rede local, Codex e Gemini, atualização automática com canal publicado.

## Compilar a partir do código

Requer Node 20+ com npm, Rust (MSVC) e as Build Tools do Visual Studio 2022.

```powershell
npm install
npm run tauri dev      # desenvolvimento
npm run tauri build    # gera o instalador em src-tauri/target/release/bundle/nsis/
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
npx tsc --noEmit
npx vitest run
```

## Release

O processo de versão, assinatura e publicação (manual) está em [`docs/release.md`](docs/release.md).

## Licença

[MIT](LICENSE). Componentes de terceiros e suas licenças estão em [`THIRD_PARTY.md`](THIRD_PARTY.md).
