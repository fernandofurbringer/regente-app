# Release do Regente

Este guia cobre a atualização automática assinada. O script `scripts/release.ps1` gera o instalador e o `latest.json`; a publicação é manual.

## Chave de assinatura (uma vez)

```powershell
npx tauri signer generate -w "$env:USERPROFILE\.tauri\regente.key" -p ""
```

- `regente.key` (privada) fica em `%USERPROFILE%\.tauri\` e **nunca** entra no repositório. Se a perda ocorrer, atualizações já instaladas não recebem novas versões assinadas.
- `regente.key.pub` (pública) é a linha base64 que vai em `src-tauri/tauri.conf.json` > `plugins.updater.pubkey`.
- Alternativa: defina `TAURI_SIGNING_PRIVATE_KEY` (conteúdo da chave) antes de rodar o script.

## Gerar uma versão

1. Suba a versão em `src-tauri/tauri.conf.json` (e em `package.json`/`Cargo.toml`, se a versão for mantida lá).
2. Rode os testes: `cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1`, `npx vitest run`, `npm run build`.
3. Gere os artefatos:

```powershell
pwsh scripts/release.ps1 -BaseUrl https://github.com/<dono>/<repo>/releases/download/v<versão> -Notes "O que mudou (pt-BR)."
```

O script produz, em `src-tauri/target/release/bundle/nsis/`, o instalador `Regente_<versão>_x64-setup.exe`, a assinatura `.sig` e o `latest.json`. Ele não faz `git push` nem cria release.

Use `-SkipBuild` para gerar o `latest.json` a partir de um build que já existe.

## Publicar (manual)

1. Crie a release no GitHub (ou no local escolhido em `-BaseUrl`).
2. Envie o instalador e o `latest.json` como arquivos da release.
3. Confira que `<BaseUrl>/latest.json` abre e que a URL do instalador responde.
4. No Regente: Configurações → Atualizações → Canal = URL do `latest.json` publicado.

## Como o usuário recebe

- "Procurar agora" consulta o canal e mostra a versão e as novidades. Nada é baixado sem clique.
- "Baixar e instalar" baixa com progresso e verifica a assinatura antes de instalar.
- No Windows, a instalação reinicia o Regente sozinha ao terminar (não há botão de reinício). As configurações e os dados ficam no mesmo lugar.
- Na primeira abertura da nova versão, a janela "Novidades" mostra as notas da release.

Sem canal configurado (campo vazio), a verificação responde "Sem canal configurado" e não usa a rede.
