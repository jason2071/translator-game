import { useState } from "react";
import { useStore } from "./store";
import ImportView from "./views/ImportView";
import GridView from "./views/GridView";
import GlossaryView from "./views/GlossaryView";
import LintPanel from "./views/LintPanel";
import SettingsView from "./views/SettingsView";
import TranslateBar from "./views/TranslateBar";
import { Sidebar } from "./components/Sidebar";
import { Modal } from "./components/Modal";

type Panel = "none" | "glossary" | "lint" | "settings";

export default function App() {
  const project = useStore((s) => s.project);
  const [panel, setPanel] = useState<Panel>("none");
  const [collapsed, setCollapsed] = useState(false);
  if (!project) return <ImportView />;
  return (
    <div className={`app${collapsed ? " collapsed" : ""}`}>
      <Sidebar
        openPanel={setPanel}
        collapsed={collapsed}
        onToggleCollapse={() => setCollapsed((c) => !c)}
      />
      <div className="main">
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
