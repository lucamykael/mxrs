import type { Manifest } from '@/types/model';

type AppHeaderProps = { project: Manifest['project'] };

export function AppHeader({ project }: AppHeaderProps) {
  return (
    <header>
      <h1>{project.name}</h1>
      <small>Mendix {project.mendix_version}</small>
    </header>
  );
}
