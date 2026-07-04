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
import TranslateBar from "./views/TranslateBar";
import { Sidebar } from "./components/Sidebar";
import { Modal } from "./components/Modal";
import { UpdateBanner } from "./components/UpdateBanner";

type Panel = "none" | "glossary" | "lint" | "settings";

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

  // Warn before quitting while a translation is still running. Finished batches
  // are already persisted; the confirm just stops the user losing the rest.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    getCurrentWindow()
      .onCloseRequested(async (event) => {
        if (useTranslation.getState().active !== null) {
          const quit = await ask(
            "A translation is still running. Finished batches are already saved, but the rest will stop. Quit anyway?",
            { title: "Quit while translating?", kind: "warning" }
          );
          if (!quit) event.preventDefault();
        }
      })
      .then((fn) => (unlisten = fn))
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  if (!project) return <ImportView />;
  return (
    <div className={`app${collapsed ? " collapsed" : ""}`}>
      <Sidebar
        openPanel={setPanel}
        collapsed={collapsed}
        onToggleCollapse={() => setCollapsed((c) => !c)}
      />
      <div className="main">
        <UpdateBanner />
        <TranslateBar openSettings={() => setPanel("settings")} />
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
    </div>
  );
}
