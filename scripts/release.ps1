<#
.SYNOPSIS
  Gera o instalador NSIS assinado e o latest.json do Regente. NÃO publica nada.

.DESCRIPTION
  1. Lê a versão de src-tauri/tauri.conf.json.
  2. Lê a chave privada de $env:TAURI_SIGNING_PRIVATE_KEY ou, se não existir, de %USERPROFILE%\.tauri\regente.key
     (fora do repositório). Sem chave, o script para.
  3. Roda `npm run tauri build` com createUpdaterArtifacts (por --config, sem alterar tauri.conf.json),
     salvo com -SkipBuild (usa um build que já existe).
  4. Escreve latest.json ao lado do instalador, com a assinatura (.sig) e a URL pública informada.
  5. Imprime os passos para publicar. A publicação é manual: este script nunca faz git push nem cria release.

.PARAMETER BaseUrl
  Pasta pública onde o instalador será publicado, sem barra final
  (ex.: https://github.com/<dono>/<repo>/releases/download/v1.0.0).

.PARAMETER Notes
  Novidades desta versão (pt-BR). Aparecem no "Novidades" depois da atualização.

.PARAMETER SkipBuild
  Não roda o build; só gera o latest.json a partir do instalador que já existe.

.EXAMPLE
  pwsh scripts/release.ps1 -BaseUrl https://github.com/dono/regente/releases/download/v1.0.0 -Notes "Primeira versão."

.NOTES
  Chave: gere uma vez com `npx tauri signer generate -w "$env:USERPROFILE\.tauri\regente.key" -p ""`.
  A chave pública (regente.key.pub) vai em src-tauri/tauri.conf.json > plugins.updater.pubkey.
  Guarde a chave privada fora do repositório: se perder, atualizações antigas não conseguem ser assinadas de novo.
#>
param(
  [Parameter(Mandatory = $true)][string]$BaseUrl,
  [Parameter(Mandatory = $true)][string]$Notes,
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$conf = Get-Content -Raw -Encoding utf8 "src-tauri/tauri.conf.json" | ConvertFrom-Json
$version = $conf.version
$BaseUrl = $BaseUrl.TrimEnd("/")

# 2. chave privada: variável de ambiente ou arquivo fora do repositório
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
  $keyPath = Join-Path $env:USERPROFILE ".tauri\regente.key"
  if (-not (Test-Path $keyPath)) {
    throw "Chave de assinatura não encontrada. Defina TAURI_SIGNING_PRIVATE_KEY ou gere com: npx tauri signer generate -w `"$keyPath`" -p `"`""
  }
  $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw $keyPath
}

# 3. build do instalador NSIS com artefatos de atualização (o arquivo tauri.conf.json não muda)
if (-not $SkipBuild) {
  # the config override goes in a temp file, passed by path (no quoting trouble with inline JSON)
  $cfgPath = Join-Path ([IO.Path]::GetTempPath()) "regente-release-config.json"
  [IO.File]::WriteAllText($cfgPath, '{"bundle":{"createUpdaterArtifacts":true}}', [Text.UTF8Encoding]::new($false))
  try {
    npm run tauri build -- --config $cfgPath
    if ($LASTEXITCODE -ne 0) { throw "build falhou (exit $LASTEXITCODE)" }
  } finally {
    Remove-Item $cfgPath -ErrorAction SilentlyContinue
  }
}

# 4. instalador e assinatura
# o cargo respeita CARGO_TARGET_DIR (o Regente aponta para um cache compartilhado entre worktrees)
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "src-tauri/target" }
$nsis = Join-Path $target "release/bundle/nsis"
$installer = Get-ChildItem -Path $nsis -Filter "*_${version}_x64-setup.exe" -File | Select-Object -First 1
if (-not $installer) { throw "instalador da versão $version não encontrado em $nsis" }
$sigPath = "$($installer.FullName).sig"
if (-not (Test-Path $sigPath)) { throw "assinatura não encontrada: $sigPath (o build foi feito com createUpdaterArtifacts?)" }
$signature = (Get-Content -Raw $sigPath).Trim()

$latest = [ordered]@{
  version   = $version
  notes     = $Notes
  pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  platforms = [ordered]@{
    "windows-x86_64" = [ordered]@{
      signature = $signature
      url       = "$BaseUrl/$($installer.Name)"
    }
  }
}
$latestPath = Join-Path $nsis "latest.json"
[IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))

Write-Host ""
Write-Host "Pronto (nada foi publicado):"
Write-Host "  instalador: $($installer.FullName)"
Write-Host "  assinatura: $sigPath"
Write-Host "  latest.json: $latestPath"
Write-Host ""
Write-Host "Para publicar (manual):"
Write-Host "  1. Crie a release v$version no GitHub (ou no lugar escolhido para -BaseUrl)."
Write-Host "  2. Envie o instalador e o latest.json como arquivos da release (o .sig não precisa ser enviado)."
Write-Host "  3. Confira que $BaseUrl/latest.json abre e que a URL do instalador responde."
Write-Host "  4. No Regente: Configurações -> Atualizações -> Canal = URL do latest.json publicado."
