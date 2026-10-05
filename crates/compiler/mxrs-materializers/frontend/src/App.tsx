import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import type { DataObject } from '@/api/data';
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
  const [route, openRoute] = useHashRoute();
  const [sidebar, setSidebar] = useState<boolean>();
  // What each page was given when it was opened, by the page's name.
  const [given, setGiven] = useState<Record<string, Record<string, DataObject>>>({});
  const [changes, setChanges] = useState(0);
  const [problem, setProblem] = useState<string>();
  // A layout that draws no menu of its own leaves the application's to the shell.
  const [menuDrawn, setMenuDrawn] = useState(true);
  useEffect(() => {
    setMenuDrawn(document.querySelector('.mx-navigationtree') !== null);
  }, [route, manifest]);
  // The page a button is opening: a page reached any other way — the
  // address bar, the browser's history — was given nothing.
  const opening = useRef<string>(undefined);
  const open = useCallback(
    (page: string, objects: Record<string, DataObject> = {}) => {
      opening.current = page;
      setGiven((all) => ({ ...all, [page]: objects }));
      openRoute(page);
    },
    [openRoute],
  );
  useEffect(() => {
    setProblem(undefined);
    if (opening.current !== route) setGiven((all) => ({ ...all, [route]: {} }));
    opening.current = undefined;
  }, [route]);
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
      <Shell.Provider
        value={{
          items: profile?.items || [],
          current: qualified,
          open,
          given: given[qualified] || {},
          changes,
          changed: () => {
            setProblem(undefined);
            setChanges((count) => count + 1);
          },
          fail: (error) => setProblem(error instanceof Error ? error.message : String(error)),
          form: findForm,
          sidebar,
          setSidebar,
        }}
      >
        {problem ? (
          <div role="alert" className="mxrs-failure" onClick={() => setProblem(undefined)}>
            {problem}
          </div>
        ) : null}
        {menuDrawn ? null : (
          <div className="mxrs-menu">
            <Navigation items={profile?.items || []} onOpen={open} />
          </div>
        )}
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
