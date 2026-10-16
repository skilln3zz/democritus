# Starts `tauri dev` in test mode: all app data goes to .e2e\ inside the project
# (never the operator's real data or D:\Democritus), and the window exposes the
# DevTools protocol on 127.0.0.1:9222 for scripts/e2e.mjs.
#   -Fresh       wipe .e2e\ first
#   -Legacy      also create a "Local Qwen" database fixture to test migration
#   -ExtraArgs   more WebView2 switches (e.g. --log-net-log=... to audit traffic)
#   -RealHome    use the real Democritus home folder (D:\Democritus) instead of .e2e\home
param([switch]$Fresh, [switch]$Legacy, [switch]$RealHome, [string]$ExtraArgs = '')

$root = Split-Path -Parent $PSScriptRoot
$e2e = Join-Path $root '.e2e'
if ($Fresh -and (Test-Path $e2e)) { Remove-Item -Recurse -Force $e2e }
New-Item -ItemType Directory -Force (Join-Path $e2e 'data') | Out-Null

$env:DEMOCRITUS_DATA_DIR = Join-Path $e2e 'data'
if ($RealHome) { Remove-Item Env:DEMOCRITUS_HOME -ErrorAction SilentlyContinue } else { $env:DEMOCRITUS_HOME = Join-Path $e2e 'home' }
# Same privacy flags as the real app (tauri.conf.json), plus the DevTools port.
$conf = Get-Content (Join-Path $root 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "$($conf.app.windows[0].additionalBrowserArgs) --remote-debugging-port=9222 $ExtraArgs".Trim()
if ($Legacy) {
    $legacyDb = Join-Path $e2e 'legacy\local-qwen.db'
    New-Item -ItemType Directory -Force (Split-Path $legacyDb) | Out-Null
    if (-not (Test-Path $legacyDb)) { python -I (Join-Path $PSScriptRoot 'make-legacy-db.py') $legacyDb }
    $env:DEMOCRITUS_LEGACY_DB = $legacyDb
}
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path
Set-Location $root
npm run tauri dev
