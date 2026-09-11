# Thai TMP font fallback for Unity (BepInEx 6 Mono) games

`ThaiFontFallback.cs` compiles to a tiny BepInEx plugin that loads a
TextMeshPro font asset bundle and registers it as a **global TMP fallback**.
Latin text keeps the game's own fonts; glyphs they lack (Thai) fall through to
Arial Unicode instead of rendering as tofu boxes.

Designed for Unity 6000 Mono games whose launcher already ships BepInEx 6
(Rebirth Pub does). XUnity.AutoTranslator is NOT required — and its official
builds don't support BepInEx 6 Mono anyway.

## Font bundle source

Community pack `TMP_Font_AssetBundles` (the XUnity.AutoTranslator release
asset), file `arialuni_sdf_u6000` — the bundle built for Unity 6000. One bundle
carries a TMP font asset for Arial Unicode (Thai + CJK + Latin coverage).

## Compile

```bat
C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe /nologo /target:library ^
  /langversion:5 /out:ThaiFontFallback.dll ^
  /r:"<game>\BepInEx\core\BepInEx.Core.dll" ^
  /r:"<game>\BepInEx\core\BepInEx.Unity.Mono.dll" ^
  /r:"<game>\Rebirth Pub_Data\Managed\UnityEngine.dll" ^
  /r:"<game>\Rebirth Pub_Data\Managed\UnityEngine.CoreModule.dll" ^
  /r:"<game>\Rebirth Pub_Data\Managed\UnityEngine.AssetBundleModule.dll" ^
  /r:"<game>\Rebirth Pub_Data\Managed\Unity.TextMeshPro.dll" ^
  /r:"<game>\Rebirth Pub_Data\Managed\netstandard.dll" ^
  ThaiFontFallback.cs
```

Notes: `BaseUnityPlugin` lives in namespace `BepInEx.Unity.Mono` on BepInEx 6
(not `BepInEx`), and `BepInEx.Unity.Mono.dll` references the `UnityEngine`
facade assembly — reference BOTH `UnityEngine.dll` and
`UnityEngine.CoreModule.dll`. Source is kept C# 5 so the built-in `csc.exe`
works without an SDK.

## Install (per game)

1. `BepInEx/plugins/TMP_Font_AssetBundles/arialuni_sdf_u6000` ← the bundle
2. `BepInEx/plugins/ThaiFontFallback.dll`

Launch once; `BepInEx/LogOutput.log` reports
`[ThaiFont] arialuni_sdf_u6000: found N font asset(s), added N global TMP fallback(s)`.
