import { useStore } from "../store";
import { Icon } from "./Icon";

export function ProjectOverview({
  onOpenGlossary,
  onOpenSettings,
}: {
  onOpenGlossary: () => void;
  onOpenSettings: () => void;
}) {
  const project = useStore((s) => s.project)!;
  const stats = useStore((s) => s.stats);

  const pathSegs = project.root.split(/[\\/]/).filter(Boolean);
  const gameName = pathSegs[pathSegs.length - 1] ?? project.root;
  const done = stats ? stats.translated + stats.reviewed : 0;
  const total = stats?.total ?? 0;

  return (
    <section className="project-overview">
      <div className="project-heading">
        <div>
          <h1>{gameName}</h1>
          <p className="project-engine">{project.engineName}</p>
        </div>
        <div className="project-overview-actions">
          <button className="ghost" onClick={onOpenGlossary}>
            <Icon name="glossary" size={15} /> Glossary
          </button>
          <button className="ghost" onClick={onOpenSettings}>
            <Icon name="settings" size={15} /> Settings
          </button>
        </div>
      </div>
      <div className="project-stats" aria-label="Project progress">
        <strong>{done.toLocaleString()} / {total.toLocaleString()}</strong>
        <span>Done</span>
      </div>
    </section>
  );
}
