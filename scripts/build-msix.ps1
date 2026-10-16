<#
.SYNOPSIS
  Assemble le package MSIX « Microsoft Store » d'OCADE Dictée à partir d'un
  build Tauri compilé avec la feature `store`.

.DESCRIPTION
  Reproduit l'arborescence que l'installeur NSIS dépose sur le disque
  (handy.exe, DLL transcribe-cpp, yt-dlp.exe, resources\) dans un dossier de
  travail, y ajoute le manifeste (src-tauri/msix/AppxManifest.xml, valeurs
  substituées) et les icônes, puis appelle makeappx.exe du SDK Windows.

  Le package produit n'est PAS signé : c'est le Store qui le signe lors de la
  soumission. Pour l'installer localement (test), -SelfSign crée un certificat
  auto-signé, signe le package et explique comment le faire accepter par Windows.

  Pré-requis : SDK Windows 10/11 (makeappx.exe, signtool.exe), et un build
  préalable :  bun run tauri build --no-bundle --features store

.EXAMPLE
  .\scripts\build-msix.ps1 -IdentityName "OCADE.OCADEDictee" -Publisher "CN=xxxxxxxx-xxxx-..." -PublisherDisplayName "OCADE"

.EXAMPLE
  .\scripts\build-msix.ps1 -SelfSign    # test local, identité factice
#>
[CmdletBinding()]
param(
  # Identité du produit (Partner Center → Gestion des produits → Identité du produit).
  [string]$IdentityName = $env:OCADE_STORE_IDENTITY_NAME,
  [string]$Publisher = $env:OCADE_STORE_PUBLISHER,
  [string]$PublisherDisplayName = $env:OCADE_STORE_PUBLISHER_DISPLAY_NAME,
  # Version à 3 composants ; lue dans src-tauri/tauri.conf.json par défaut.
  [string]$Version = "",
  # Profil cargo du build (`release` pour `tauri build`, `debug` pour un essai).
  [string]$Profile = "release",
  # Dossier de sortie du .msix.
  [string]$OutDir = "",
  # Test local : certificat auto-signé + signature du package.
  [switch]$SelfSign,
  # 4e composant de la version MSIX (0 en production). À incrémenter pour
  # réinstaller un package de test : Windows refuse un package de même version
  # au contenu différent.
  [int]$Revision = 0
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $root "src-tauri"

if (-not $IdentityName) { $IdentityName = "OCADE.OCADEDicteeTest" }
if (-not $Publisher) { $Publisher = "CN=OCADE-TEST" }
if (-not $PublisherDisplayName) { $PublisherDisplayName = "OCADE" }
if (-not $Version) {
  $conf = Get-Content (Join-Path $tauriDir "tauri.conf.json") -Raw | ConvertFrom-Json
  $Version = $conf.version
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version attendue au format x.y.z, reçu « $Version »." }
$msixVersion = "$Version.$Revision"
if (-not $OutDir) { $OutDir = Join-Path $tauriDir "target\$Profile\bundle\msix" }

# --- Outils du SDK Windows --------------------------------------------------
$kitsBin = "C:\Program Files (x86)\Windows Kits\10\bin"
$sdkBin = Get-ChildItem $kitsBin -Directory -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -match '^10\.' -and (Test-Path (Join-Path $_.FullName "x64\makeappx.exe")) } |
  Sort-Object Name -Descending | Select-Object -First 1
if (-not $sdkBin) { throw "makeappx.exe introuvable : installez le SDK Windows 10/11 (Build Tools, workload C++)." }
$makeappx = Join-Path $sdkBin.FullName "x64\makeappx.exe"
$signtool = Join-Path $sdkBin.FullName "x64\signtool.exe"

# --- Fichiers du build ------------------------------------------------------
$exe = Join-Path $tauriDir "target\$Profile\handy.exe"
if (-not (Test-Path $exe)) { throw "Binaire absent ($exe). Lancez d'abord : bun run tauri build --no-bundle --features store" }
$libs = Join-Path $tauriDir "transcribe-libs"
if (-not (Test-Path (Join-Path $libs "transcribe.dll"))) { throw "DLL transcribe-cpp absentes ($libs) : le build Rust ne les a pas mises en place." }
$ytdlp = Join-Path $tauriDir "binaries\yt-dlp-x86_64-pc-windows-msvc.exe"
if (-not (Test-Path $ytdlp)) { throw "Sidecar yt-dlp absent ($ytdlp). Lancez : bun scripts/fetch-yt-dlp.ts" }

# --- Assemblage -------------------------------------------------------------
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("ocade-msix-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $stage | Out-Null
Write-Host "Assemblage dans $stage"

Copy-Item $exe (Join-Path $stage "handy.exe")
Get-ChildItem $libs -Filter *.dll | Copy-Item -Destination $stage
Copy-Item $ytdlp (Join-Path $stage "yt-dlp.exe")
Copy-Item (Join-Path $tauriDir "resources") (Join-Path $stage "resources") -Recurse

$assets = Join-Path $stage "Assets"
New-Item -ItemType Directory -Force $assets | Out-Null
foreach ($icon in @("StoreLogo.png", "Square150x150Logo.png", "Square44x44Logo.png", "Square71x71Logo.png")) {
  Copy-Item (Join-Path $tauriDir "icons\$icon") (Join-Path $assets $icon)
}

$manifest = Get-Content (Join-Path $tauriDir "msix\AppxManifest.xml") -Raw
$manifest = $manifest.Replace("{{IDENTITY_NAME}}", $IdentityName).Replace("{{PUBLISHER}}", $Publisher).Replace("{{PUBLISHER_DISPLAY_NAME}}", $PublisherDisplayName).Replace("{{VERSION}}", $msixVersion)
[System.IO.File]::WriteAllText((Join-Path $stage "AppxManifest.xml"), $manifest, (New-Object System.Text.UTF8Encoding $false))

New-Item -ItemType Directory -Force $OutDir | Out-Null
$msix = Join-Path $OutDir "OCADE.Dictee_${Version}_x64.msix"
if (Test-Path $msix) { Remove-Item $msix -Force }
& $makeappx pack /d $stage /p $msix /o | Out-Host
if ($LASTEXITCODE -ne 0) { throw "makeappx a échoué (code $LASTEXITCODE)." }
Remove-Item $stage -Recurse -Force

# --- Signature de test (facultative) ---------------------------------------
if ($SelfSign) {
  $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq $Publisher -and $_.NotAfter -gt (Get-Date) } | Select-Object -First 1
  if (-not $cert) {
    Write-Host "Création d'un certificat auto-signé pour $Publisher"
    $cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature `
      -FriendlyName "OCADE Dictée (test MSIX)" -CertStoreLocation "Cert:\CurrentUser\My" `
      -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
  }
  $cerPath = Join-Path $OutDir "ocade-msix-test.cer"
  Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
  & $signtool sign /fd SHA256 /sha1 $cert.Thumbprint $msix | Out-Host
  if ($LASTEXITCODE -ne 0) { throw "signtool a échoué (code $LASTEXITCODE)." }
  Write-Host ""
  Write-Host "Package signé avec un certificat de TEST. Pour l'installer sur ce poste (une fois, en administrateur) :"
  Write-Host "  Import-Certificate -FilePath `"$cerPath`" -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
  Write-Host "puis :"
  Write-Host "  Add-AppxPackage `"$msix`""
}

Write-Host ""
Write-Host "Package : $msix"
Write-Host "Identité : $IdentityName / $Publisher / $msixVersion"
