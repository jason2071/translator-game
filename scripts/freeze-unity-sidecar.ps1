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

    * **Full (default)** — bundles numpy + scipy + PIL + freetype-py so `bake-font`
      (SDF font baking, `unity-textbl`) works in the shipped app. This is the default
      because a release frozen without it translates the text but renders Thai as tofu
      in pre-baked-SDF games (e.g. NTR Soccer) — a silent-looking failure. ~60 MB larger.

    * **-Lean** — text tiers only. UnityPy's texture deps (PIL, numpy, astc_encoder,
      texture2ddecoder, etcpak) + scipy are excluded to trim ~60 MB; `rpgtl_unity.py`
      stubs the ones UnityPy imports at load so `import UnityPy` still succeeds.
      `bake-font` then exits with an actionable message. Dev-only — do NOT ship it.

  Either way the chosen profile is recorded in `rpgtl-unity.profile` beside the exe,
  and `build.rs` warns when a release build would embed a lean (or missing) sidecar.

  Requirements (build machine only, not end users):
    - Python 3.x on PATH
    - pip install UnityPy pyinstaller
    - for the default (full) profile also: pip install numpy scipy pillow freetype-py

  `--collect-data UnityPy` bundles UnityPy's own data (typetree DB), without which the
  exe fails at load.

  Run from the repo root:
    pwsh scripts/freeze-unity-sidecar.ps1          # full — SDF baking works (ship this)
    pwsh scripts/freeze-unity-sidecar.ps1 -Lean    # dev-only, ~60 MB smaller
#>
[CmdletBinding()]
param(
    [string]$Python = "python",
    # Opt OUT of the font-bake deps. Shipping a lean sidecar means `bake-font` fails and
    # Thai renders as tofu in pre-baked-SDF games, so this is for local iteration only.
    [switch]$Lean
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
    "--exclude-module", "IPython", "--exclude-module", "pytest",
    # scipy/numpy ship array-API shims that statically `import torch` (and cupy, dask,
    # jax, ...) for backends they would only ever use at runtime if installed. Static
    # analysis follows those imports, which is how a text-and-fonts helper ended up
    # bundling torch (317 MB), cv2, pyarrow, transformers and onnxruntime. Nothing on
    # our code path imports them, so cut them at the root.
    "--exclude-module", "torch", "--exclude-module", "torchvision",
    "--exclude-module", "transformers", "--exclude-module", "cv2",
    "--exclude-module", "pyarrow", "--exclude-module", "onnxruntime",
    "--exclude-module", "llvmlite", "--exclude-module", "numba",
    "--exclude-module", "jax", "--exclude-module", "cupy",
    "--exclude-module", "dask", "--exclude-module", "sparse",
    "--exclude-module", "pandas", "--exclude-module", "sympy",
    "--exclude-module", "sklearn", "--exclude-module", "lief"
)

if (-not $Lean) {
    Write-Host "Profile: full (bundles numpy/scipy/PIL/freetype for bake-font)" -ForegroundColor Cyan
    # Verify the SDF deps are importable in the build interpreter before a long freeze.
    $probe = "import numpy, scipy.ndimage, PIL.Image, freetype"
    & $Python -c $probe
    if ($LASTEXITCODE -ne 0) {
        throw "Font-bake deps missing in '$Python'. Run: $Python -m pip install numpy scipy pillow freetype-py"
    }
    # numpy needs collect-all (its DLLs + data aren't all caught by static analysis) and
    # freetype-py ships a native DLL, so it does too. scipy and PIL do NOT: only
    # `scipy.ndimage.distance_transform_edt` and `PIL.Image` are used, and collect-all
    # dragged in all of scipy (117 MB on disk) + all of Pillow, which is what made the
    # frozen exe 405 MB. Naming the imports lets PyInstaller's own hooks pull just those
    # subpackages and their binaries.
    # Decoding the atlas Texture2D pulls in UnityPy's native texture/audio deps (their DLLs
    # aren't caught by --collect-submodules), so collect-all them too or the read fails
    # (e.g. "Failed to load fmod.dll") and every font is skipped -> no baking.
    $pyi += @(
        "--collect-all", "numpy", "--collect-all", "freetype",
        "--hidden-import", "scipy.ndimage", "--hidden-import", "PIL.Image",
        "--collect-all", "fmod_toolkit", "--collect-all", "astc_encoder",
        "--collect-all", "texture2ddecoder", "--collect-all", "etcpak",
        # astc-encoder-py + etcpak depend on archspec, whose CPU database is a JSON
        # data file. Without it every font read raises FileNotFoundError mid-bake and
        # bake-font silently baked 0 glyphs -> Thai stayed tofu in a "successful" export.
        "--collect-all", "archspec",
        "--hidden-import", "freetype"
    )
} else {
    Write-Host "Profile: lean (text tiers only; bake-font disabled - DO NOT SHIP)" -ForegroundColor Yellow
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

# Record which profile this exe is, so build.rs can refuse to ship a lean one silently.
$profileName = if ($Lean) { "lean" } else { "full" }
Set-Content -Path (Join-Path $outDir "rpgtl-unity.profile") -Value $profileName -Encoding ascii -NoNewline

$mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "Built rpgtl-unity.exe ($mb MB, profile: $profileName)." -ForegroundColor Green
Write-Host "Now run 'cargo build' (or 'pnpm tauri build') to embed it." -ForegroundColor Green
