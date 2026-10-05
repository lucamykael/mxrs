import { useMemo, useState } from 'react';

import { Shell } from '@/components/elements/context';
import { AppHeader } from '@/components/layout/AppHeader';
import { Navigation } from '@/components/layout/Navigation';
import { useHashRoute } from '@/hooks/useHashRoute';
import { useManifest } from '@/hooks/useManifest';
import { ModelPage } from '@/pages/ModelPage';
import { PageNotFound } from '@/pages/PageNotFound';
import { findForm } from '@/utils/forms';

/**
 * The application: the page the route names. A page the frontend declares
 * is drawn from its TSX, inside the layout it calls; one only the model
 * has is drawn from the manifest, inside the shell's own header and menu.
 */
export function App() {
  const { manifest, failure } = useManifest();
  const [route, open] = useHashRoute();
  const [sidebar, setSidebar] = useState<boolean>();
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

  const qualified = page?.qualified_name ?? requested;
  const declared = findForm(qualified);
  // A page is what a route opens; a layout or a snippet is drawn inside one.
  if (declared?.kind === 'page') {
    return (
      <Shell.Provider value={{ items: profile?.items || [], current: qualified, open, form: findForm, sidebar, setSidebar }}>
        {/* Keyed by the page, so nothing one page holds — a tab, a text typed — is taken for another's. */}
        <div className="mxrs-app" data-page={qualified} key={qualified}>
          {declared.document}
        </div>
      </Shell.Provider>
    );
  }

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
