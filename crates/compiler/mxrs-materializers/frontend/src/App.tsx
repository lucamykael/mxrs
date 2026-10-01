import { useMemo } from 'react';

import { AppHeader } from '@/components/layout/AppHeader';
import { Navigation } from '@/components/layout/Navigation';
import { useHashRoute } from '@/hooks/useHashRoute';
import { useManifest } from '@/hooks/useManifest';
import { ModelPage } from '@/pages/ModelPage';
import { PageNotFound } from '@/pages/PageNotFound';

/** The application shell: header, navigation, and the page the route names. */
export function App() {
  const { manifest, failure } = useManifest();
  const [route, open] = useHashRoute();
  const pages = useMemo(
    () => (manifest ? manifest.modules.flatMap((module) => module.pages) : []),
    [manifest],
  );

  if (failure) return <main role="alert">Unable to load application: {failure}</main>;
  if (!manifest) return <main aria-busy="true">Loading…</main>;

  const profile = manifest.navigation.profiles[0];
  const requested = route || profile?.home_page || pages[0]?.qualified_name || '';
  const page = pages.find(
    (candidate) => candidate.qualified_name === requested || candidate.name === requested,
  );

  return (
    <div className="mxrs-app">
      <AppHeader project={manifest.project} />
      <div className="mxrs-shell">
        <Navigation items={profile?.items || []} onOpen={open} />
        <main data-page={page?.qualified_name}>
          {page ? <ModelPage page={page} /> : <PageNotFound requested={requested} />}
        </main>
      </div>
    </div>
  );
}
