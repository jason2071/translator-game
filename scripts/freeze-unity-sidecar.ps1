<#
.SYNOPSIS
  Freeze the Unity (Naninovel) helper `rpgtl_unity.py` into a standalone exe that
  the `unity` engine embeds (see `src-tauri/src/engine/unity.rs`).

.DESCRIPTION
  The engine reads/writes Naninovel managed text via UnityPy. Unity games ship no
  Python, so we bundle a frozen interpreter. This script runs PyInstaller to build
  a one-file exe into `src-tauri/resources/unity/rpgtl-unity.exe`, which `build.rs`
  then embeds (`include_bytes!`) into the Rust binary. When the exe is absent the
  engine falls back to system Python, so this step is only needed to produce a
  shippable build with no system-Python dependency.

  Requirements (build machine only, not end users):
    - Python 3.x on PATH
    - pip install UnityPy pyinstaller

  We only touch `TextAsset`, never decode an image, so UnityPy's texture deps
  (PIL, numpy, astc_encoder, texture2ddecoder, etcpak) are excluded — trimming the
  frozen size. `rpgtl_unity.py` stubs those imports under a frozen build so
  `import UnityPy` still succeeds. `--collect-data UnityPy` bundles UnityPy's own
  data resources (typetree DB), without which the exe fails at load.

  Run from the repo root:  pwsh scripts/freeze-unity-sidecar.ps1
#>
[CmdletBinding()]
param(
    [string]$Python = "python"
)

$ErrorActionPreference = "Stop"

$repo   = Split-Path -Parent $PSScriptRoot
$src    = Join-Path $repo "src-tauri/resources/unity/rpgtl_unity.py"
$outDir = Join-Path $repo "src-tauri/resources/unity"
$work   = Join-Path $env:TEMP "rpgtl-unity-freeze"

if (-not (Test-Path $src)) { throw "sidecar not found: $src" }

Write-Host "Freezing $src -> $outDir/rpgtl-unity.exe" -ForegroundColor Cyan

# PyInstaller writes progress to stderr; Windows PowerShell wraps native stderr as
# error records, so with $ErrorActionPreference='Stop' the first line would abort
# the build. Relax it for the native call and gate on the real exit code instead.
$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $Python -m PyInstaller `
    --onefile --name rpgtl-unity --noconfirm --clean `
    --distpath $outDir `
    --workpath (Join-Path $work "build") `
    --specpath $work `
    --collect-data UnityPy --collect-submodules UnityPy `
    --exclude-module PIL --exclude-module numpy `
    --exclude-module astc_encoder --exclude-module texture2ddecoder `
    --exclude-module etcpak --exclude-module matplotlib `
    --exclude-module tkinter --exclude-module scipy `
    --exclude-module IPython --exclude-module pytest `
    $src 2>&1 | ForEach-Object { "$_" }
$code = $LASTEXITCODE
$ErrorActionPreference = $prev
if ($code -ne 0) { throw "PyInstaller failed (exit $code)" }

$exe = Join-Path $outDir "rpgtl-unity.exe"
if (-not (Test-Path $exe)) { throw "PyInstaller did not produce $exe" }

$mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "Built rpgtl-unity.exe ($mb MB)." -ForegroundColor Green
Write-Host "Now run 'cargo build' (or 'pnpm tauri build') to embed it." -ForegroundColor Green
