# Starts `tauri dev` in test mode: all app data goes to .e2e\ inside the project
# (never the operator's real data or D:\Democritus), and the window exposes the
# DevTools protocol on 127.0.0.1:9222 for scripts/e2e.mjs.
#   -Fresh       wipe .e2e\ first
#   -Legacy      also create a "Local Qwen" database fixture to test migration
param([switch]$Fresh, [switch]$Legacy)

$root = Split-Path -Parent $PSScriptRoot
$e2e = Join-Path $root '.e2e'
if ($Fresh -and (Test-Path $e2e)) { Remove-Item -Recurse -Force $e2e }
New-Item -ItemType Directory -Force (Join-Path $e2e 'data') | Out-Null

$env:DEMOCRITUS_DATA_DIR = Join-Path $e2e 'data'
$env:DEMOCRITUS_HOME = Join-Path $e2e 'home'
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9222'
if ($Legacy) {
    $legacyDb = Join-Path $e2e 'legacy\local-qwen.db'
    New-Item -ItemType Directory -Force (Split-Path $legacyDb) | Out-Null
    if (-not (Test-Path $legacyDb)) { python -I (Join-Path $PSScriptRoot 'make-legacy-db.py') $legacyDb }
    $env:DEMOCRITUS_LEGACY_DB = $legacyDb
}
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
Set-Location $root
npm run tauri dev
