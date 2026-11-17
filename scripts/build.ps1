$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    cargo build -p openidea-plugin-local-ai-assistant --target wasm32-wasip2 --release

    $src = Join-Path $repoRoot "target\wasm32-wasip2\release\openidea_plugin_local_ai_assistant.wasm"
    if (-not (Test-Path $src)) {
        throw "Compiled WASM artifact not found: $src"
    }

    $destDir = Join-Path $repoRoot "plugins\ai-assistant\dist"
    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    $dst = Join-Path $destDir "plugin.wasm"
    Copy-Item -Force $src $dst
    $size = (Get-Item $dst).Length

    Write-Host ("Built {0} ({1:N0} bytes)" -f $dst, $size)
    Write-Host ("Install in OpenIDEA: Settings > Plugins > Install Plugin from Disk... -> {0}" -f (Join-Path $repoRoot "plugins\ai-assistant"))
}
finally {
    Pop-Location
}
