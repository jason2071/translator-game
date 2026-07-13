<#
.SYNOPSIS
  Freeze the Unity helper `rpgtl_unity.py` into a standalone exe that the `unity`
  engines embed (see `src-tauri/src/engine/unity.rs`).

.DESCRIPTION
  The Unity engines read/write text via UnityPy, and Unity games ship no Python, so we
  bundle a frozen interpreter. This runs PyInstaller to build a one-file exe into
  `src-tauri/resources/unity/rpgtl-unity.exe`, which `build.rs` embeds (`include_bytes!`)
  into the Rust binary. When the exe is absent the engine falls back to system Python, so
  this step is only needed to produce a shippable build with no system-Python dependency.

  Two profiles:

    * **Lean (default)** — text tiers only. UnityPy's texture deps (PIL, numpy,
      astc_encoder, texture2ddecoder, etcpak) + scipy are excluded to trim ~60 MB;
      `rpgtl_unity.py` stubs the ones UnityPy imports at load so `import UnityPy` still
      succeeds. `bake-font` (SDF font baking, `unity-textbl`) exits with an actionable
      message under this build — text translates but Thai renders as tofu without a font.

    * **-WithFontBake** — also bundles numpy + scipy + PIL + freetype-py so `bake-font`
      works in the shipped app. Needed to release support for pre-baked-SDF Unity games
      (e.g. NTR Soccer / `unity-textbl`); the exe is ~60 MB larger.

  Requirements (build machine only, not end users):
    - Python 3.x on PATH
    - pip install UnityPy pyinstaller
    - for -WithFontBake also: pip install numpy scipy pillow freetype-py

  `--collect-data UnityPy` bundles UnityPy's own data (typetree DB), without which the
  exe fails at load.

  Run from the repo root:
    pwsh scripts/freeze-unity-sidecar.ps1                 # lean
    pwsh scripts/freeze-unity-sidecar.ps1 -WithFontBake   # fat (SDF baking works)
#>
[CmdletBinding()]
param(
    [string]$Python = "python",
    [switch]$WithFontBake
)

$ErrorActionPreference = "Stop"

$repo   = Split-Path -Parent $PSScriptRoot
$src    = Join-Path $repo "src-tauri/resources/unity/rpgtl_unity.py"
$outDir = Join-Path $repo "src-tauri/resources/unity"
$work   = Join-Path $env:TEMP "rpgtl-unity-freeze"

if (-not (Test-Path $src)) { throw "sidecar not found: $src" }

# Base args + the modules never needed either way.
$pyi = @(
    "--onefile", "--name", "rpgtl-unity", "--noconfirm", "--clean",
    "--distpath", $outDir,
    "--workpath", (Join-Path $work "build"),
    "--specpath", $work,
    "--collect-data", "UnityPy", "--collect-submodules", "UnityPy",
    "--exclude-module", "matplotlib", "--exclude-module", "tkinter",
    "--exclude-module", "IPython", "--exclude-module", "pytest"
)

if ($WithFontBake) {
    Write-Host "Profile: WithFontBake (bundles numpy/scipy/PIL/freetype for bake-font)" -ForegroundColor Cyan
    # Verify the SDF deps are importable in the build interpreter before a long freeze.
    $probe = "import numpy, scipy.ndimage, PIL.Image, freetype"
    & $Python -c $probe
    if ($LASTEXITCODE -ne 0) {
        throw "Font-bake deps missing in '$Python'. Run: $Python -m pip install numpy scipy pillow freetype-py"
    }
    # scipy/numpy pull in data + many submodules PyInstaller misses via static analysis;
    # collect-all is the robust way. freetype-py ships a native DLL — collect-all grabs it.
    $pyi += @(
        "--collect-all", "numpy", "--collect-all", "scipy",
        "--collect-all", "PIL", "--collect-all", "freetype",
        "--hidden-import", "freetype"
    )
} else {
    Write-Host "Profile: lean (text tiers only; bake-font disabled)" -ForegroundColor Cyan
    $pyi += @(
        "--exclude-module", "PIL", "--exclude-module", "numpy",
        "--exclude-module", "astc_encoder", "--exclude-module", "texture2ddecoder",
        "--exclude-module", "etcpak", "--exclude-module", "scipy"
    )
}

Write-Host "Freezing $src -> $outDir/rpgtl-unity.exe" -ForegroundColor Cyan

# PyInstaller writes progress to stderr; Windows PowerShell wraps native stderr as error
# records, so with $ErrorActionPreference='Stop' the first line would abort the build.
# Relax it for the native call and gate on the real exit code instead.
$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $Python -m PyInstaller @pyi $src 2>&1 | ForEach-Object { "$_" }
$code = $LASTEXITCODE
$ErrorActionPreference = $prev
if ($code -ne 0) { throw "PyInstaller failed (exit $code)" }

$exe = Join-Path $outDir "rpgtl-unity.exe"
if (-not (Test-Path $exe)) { throw "PyInstaller did not produce $exe" }

$mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "Built rpgtl-unity.exe ($mb MB)." -ForegroundColor Green
Write-Host "Now run 'cargo build' (or 'pnpm tauri build') to embed it." -ForegroundColor Green
