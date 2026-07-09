import { useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask } from "@tauri-apps/plugin-dialog";
import { api } from "./ipc";
import { useStore } from "./store";
import { useTranslation } from "./translation";
import ImportView from "./views/ImportView";
import GridView from "./views/GridView";
import GlossaryView from "./views/GlossaryView";
import LintPanel from "./views/LintPanel";
import SettingsView from "./views/SettingsView";
import ErrorsPanel from "./views/ErrorsPanel";
import TranslateBar from "./views/TranslateBar";
import { useErrors } from "./errors";
import { Sidebar } from "./components/Sidebar";
import { Modal } from "./components/Modal";
import { UpdateBanner } from "./components/UpdateBanner";

type Panel = "none" | "glossary" | "lint" | "settings" | "errors";

// The window close guard is registered exactly once for the app's lifetime.
// See the effect below — unlistening an onCloseRequested handler must be avoided.
let closeGuardRegistered = false;

export default function App() {
  const project = useStore((s) => s.project);
  const applyUnitUpdates = useStore((s) => s.applyUnitUpdates);
  const [panel, setPanel] = useState<Panel>("none");
  const [collapsed, setCollapsed] = useState(false);

  // Fill grid rows live as a Run persists each batch (like the glossary panel),
  // instead of only refreshing when the whole Run finishes.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    api.onUnitsUpdate(applyUnitUpdates).then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, [applyUnitUpdates]);

  // Collect per-unit failure reasons (which line, why) for the errors modal.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    api
      .onTranslateFailed((items) => useErrors.getState().record(items))
      .then((fn) => (unlisten = fn));
    return () => unlisten?.();
  }, []);

  // Warn before quitting while a translation is still running. Finished batches
  // are already persisted; the confirm just stops the user losing the rest.
  //
  // Registered exactly once for the app's lifetime and deliberately NOT
  // unlistened. Calling an onCloseRequested handler's unlisten() leaves the
  // window unable to close at all (tauri-apps/tauri#7119); React StrictMode's
  // dev-only mount → cleanup → mount would otherwise run that unlisten and the
  // app could no longer be closed (its process then lingers in the background).
  useEffect(() => {
    if (closeGuardRegistered) return;
    closeGuardRegistered = true;
    const win = getCurrentWindow();
    win
      .onCloseRequested(async (event) => {
        const active = useTranslation.getState().active;
        if (active === null) return; // idle → let the window close normally

        // Cancel the close SYNCHRONOUSLY first. Showing a native dialog while the
        // OS close is mid-flight deadlocks WebView2 on Windows — the message loop
        // handling the close is blocked waiting on the modal, so the app appears
        // frozen and can't be closed. With the close already prevented the loop
        // is free to show the confirm; if the user quits we force-close via
        // destroy(), which bypasses this handler (no re-entry).
        event.preventDefault();
        const quit = await ask(
          "A translation is still running. Finished batches are already saved, but the rest will stop. Quit anyway?",
          { title: "Quit while translating?", kind: "warning" }
        );
        if (quit) {
          useTranslation.getState().cancel(active);
          await win.destroy();
        }
      })
      .catch(() => {});
  }, []);

  if (!project)
    return (
      <>
        <UpdateBanner />
        <ImportView />
      </>
    );
  return (
    <div className={`app${collapsed ? " collapsed" : ""}`}>
      <Sidebar
        openPanel={setPanel}
        collapsed={collapsed}
        onToggleCollapse={() => setCollapsed((c) => !c)}
      />
      <div className="main">
        <TranslateBar onOpenErrors={() => setPanel("errors")} />
        <GridView />
      </div>

      {panel === "glossary" && (
        <Modal title="Glossary" onClose={() => setPanel("none")}>
          <GlossaryView />
        </Modal>
      )}
      {panel === "lint" && (
        <Modal title="Glossary lint" onClose={() => setPanel("none")}>
          <LintPanel onClose={() => setPanel("none")} />
        </Modal>
      )}
      {panel === "settings" && (
        <Modal title="AI providers & settings" onClose={() => setPanel("none")}>
          <SettingsView />
        </Modal>
      )}
      {panel === "errors" && (
        <Modal title="Translation errors" onClose={() => setPanel("none")}>
          <ErrorsPanel onClose={() => setPanel("none")} />
        </Modal>
      )}
    </div>
  );
}
