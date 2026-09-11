// ThaiFontFallback — BepInEx 6 (Mono) plugin for Rebirth Pub (Unity 6000).
//
// Registers a THAI-capable TextMeshPro fallback font:
//   1. in the global TMP_Settings.fallbackFontAssets list, and
//   2. in the per-asset fallbackFontAssetTable of EVERY TMP font asset the
//      game loads (refreshed per scene).
//
// The font is built with TMP_FontAsset.CreateFontAsset(familyName, styleName,
// pointSize) — the TMP 3.2 overload that accepts an OS font family name
// ("Tahoma", "Leelawadee UI", …) and populates glyphs on demand. Older
// overloads do NOT work here: CreateFontAsset(Font) returns null for OS fonts
// and FontEngine.LoadFontFace(Font) reports Invalid_File_Structure, which is
// why earlier attempts showed tofu. Same approach as XUnity.AutoTranslator
// PR #854 (OverrideFontTextMeshPro=<system font name>).
//
// FontEngine may not be ready during the BepInEx bootstrap scene, so creation
// is retried on every scene load until it succeeds.
//
// All diagnostics go through the BepInEx `Logger` — Unity Debug.Log is not
// captured by this game's log writer. The plugin does nothing when the file
// `BepInEx/plugins/ThaiFontFallback.disabled` exists.
//
// Written against C# 5 so the Framework `csc.exe` compiles it with no SDK.

using System.Collections.Generic;
using System.IO;
using BepInEx;
using BepInEx.Logging;
using BepInEx.Unity.Mono;
using TMPro;
using UnityEngine;
using UnityEngine.SceneManagement;

namespace RPGTL
{
    [BepInPlugin("rpgtl.thaifont", "Thai TMP Font Fallback", "1.6.0")]
    public class ThaiFontFallback : BaseUnityPlugin
    {
        // OS fonts with full Thai coverage, most preferred first.
        private static readonly string[] ThaiOsFonts = new string[]
        {
            "Tahoma", "Leelawadee UI", "Leelawadee", "Cordia New",
            "Angsana New", "Browallia New"
        };

        private static ManualLogSource log;
        private List<TMP_FontAsset> fonts = new List<TMP_FontAsset>();
        private TMP_FontAsset osFont;

        private void Awake()
        {
            log = Logger;
            try
            {
                if (File.Exists(Path.Combine(Paths.PluginPath, "ThaiFontFallback.disabled")))
                {
                    log.LogInfo("disabled by kill-switch file — doing nothing");
                    return;
                }

                TryCreateOsFontAsset();
                if (osFont == null)
                {
                    // FontEngine is often not ready during the BepInEx
                    // bootstrap scene — retry on every scene load.
                    log.LogInfo("OS font not available yet — retrying on every scene load");
                }

                SceneManager.sceneLoaded += OnSceneLoaded;
                AddToGlobal();
                PatchLoadedFonts(null);
                log.LogInfo("ready — global fallback count: " + TMP_Settings.fallbackFontAssets.Count);
            }
            catch (System.Exception e)
            {
                if (log != null)
                {
                    log.LogError(e.ToString());
                }
            }
        }

        private void TryCreateOsFontAsset()
        {
            foreach (string name in ThaiOsFonts)
            {
                try
                {
                    TMP_FontAsset fa = TMP_FontAsset.CreateFontAsset(name, "Regular", 90);
                    if (fa == null)
                    {
                        log.LogInfo("CreateFontAsset(name) returned null for " + name);
                        continue;
                    }
                    fa.name = "ThaiOsFallback(" + name + ")";
                    osFont = fa;
                    if (!fonts.Contains(fa))
                    {
                        fonts.Insert(0, fa);
                    }
                    log.LogInfo("created dynamic TMP font from OS font: " + name
                        + " (atlasPopulationMode=" + fa.atlasPopulationMode + ")");
                    AddToGlobal();
                    PatchLoadedFonts(null);
                    return;
                }
                catch (System.Exception e)
                {
                    log.LogError("creating the OS font asset failed for " + name + ": " + e);
                }
            }
        }

        private void AddToGlobal()
        {
            List<TMP_FontAsset> list = TMP_Settings.fallbackFontAssets;
            int before = list.Count;
            foreach (TMP_FontAsset f in fonts)
            {
                if (f == null || list.Contains(f))
                {
                    continue;
                }
                list.Add(f);
            }
            if (list.Count != before)
            {
                log.LogInfo("global fallback list: " + before + " -> " + list.Count);
            }
        }

        private void OnSceneLoaded(Scene scene, LoadSceneMode mode)
        {
            if (osFont == null)
            {
                TryCreateOsFontAsset();
            }
            PatchLoadedFonts(scene.name);
        }

        // Inject into every loaded font asset's own fallback table too, so
        // fonts that bypass the global list are covered as well. Idempotent.
        private void PatchLoadedFonts(string sceneName)
        {
            if (fonts == null || fonts.Count == 0)
            {
                return;
            }
            TMP_FontAsset[] loaded = Resources.FindObjectsOfTypeAll<TMP_FontAsset>();
            int patched = 0;
            foreach (TMP_FontAsset target in loaded)
            {
                if (target == null || IsOurs(target) || target.fallbackFontAssetTable == null)
                {
                    continue;
                }
                if (!HasAny(target.fallbackFontAssetTable))
                {
                    foreach (TMP_FontAsset f in fonts)
                    {
                        target.fallbackFontAssetTable.Add(f);
                    }
                    patched++;
                }
            }
            if (patched > 0)
            {
                log.LogInfo((sceneName == null ? "startup" : "scene '" + sceneName + "'")
                    + ": injected fallback into " + patched + " of "
                    + loaded.Length + " loaded font asset(s)");
            }
        }

        private bool IsOurs(TMP_FontAsset target)
        {
            foreach (TMP_FontAsset f in fonts)
            {
                if (target == f)
                {
                    return true;
                }
            }
            return false;
        }

        private bool HasAny(List<TMP_FontAsset> table)
        {
            foreach (TMP_FontAsset f in fonts)
            {
                if (table.Contains(f))
                {
                    return true;
                }
            }
            return false;
        }
    }
}
