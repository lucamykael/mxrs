import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';

import type { DataObject } from '@/api/data';
import { Shell } from '@/components/elements/context';
import { AppHeader } from '@/components/layout/AppHeader';
import { Navigation } from '@/components/layout/Navigation';
import { useHashRoute } from '@/hooks/useHashRoute';
import { useCollections, useManifest } from '@/hooks/useManifest';
import { ModelPage } from '@/pages/ModelPage';
import { PageNotFound } from '@/pages/PageNotFound';
import { findForm, openingOf } from '@/utils/forms';

/** What `opening` holds while the application itself goes back a page. */
const BACK = '\u0000back';

/**
 * The application: the page the route names. A page the frontend declares
 * is drawn from its TSX, inside the layout it calls; one only the model
 * has is drawn from the manifest, inside the shell's own header and menu.
 */
export function App() {
  const { manifest, failure } = useManifest();
  const collections = useCollections();
  const [route, openRoute] = useHashRoute();
  const [sidebar, setSidebar] = useState<boolean>();
  // What each page was given when it was opened, by the page's name.
  const [given, setGiven] = useState<Record<string, Record<string, DataObject>>>({});
  const [changes, setChanges] = useState(0);
  const [problem, setProblem] = useState<string>();
  // What a flow told the user, until it is dismissed.
  const [notice, setNotice] = useState<{ message: string; level: string }>();
  // A layout that draws no menu of its own leaves the application's to the shell.
  const [menuDrawn, setMenuDrawn] = useState(true);
  useEffect(() => {
    setMenuDrawn(document.querySelector('.mx-navigationtree') !== null);
  }, [route, manifest]);
  // The popups open over the page, the last one on top, each with what
  // it was given. They are not routes: the address stays the page's.
  const [popups, setPopups] = useState<{ page: string; given: Record<string, DataObject> }[]>([]);
  // The page a button is opening: a page reached any other way — the
  // address bar, the browser's history — was given nothing.
  const opening = useRef<string>(undefined);
  const open = useCallback(
    (page: string, objects: Record<string, DataObject> = {}) => {
      if (openingOf(page).popup) {
        setPopups((all) => [...all, { page, given: objects }]);
        return;
      }
      setPopups([]);
      opening.current = page;
      setGiven((all) => ({ ...all, [page]: objects }));
      openRoute(page);
    },
    [openRoute],
  );
  // How many pages this application opened that going back returns from:
  // leaving a page goes back to the one before it only when it opened one,
  // and home otherwise — never out of the application.
  const depth = useRef(0);
  const leave = useCallback(() => {
    if (depth.current > 0) {
      depth.current -= 1;
      opening.current = BACK;
      history.back();
    } else openRoute('');
  }, [openRoute]);
  useEffect(() => {
    setProblem(undefined);
    setPopups([]);
    if (opening.current === route) depth.current += 1;
    // Reached another way — the browser's own back — it is one less deep.
    else if (opening.current !== BACK) depth.current = Math.max(0, depth.current - 1);
    if (opening.current !== route) setGiven((all) => ({ ...all, [route]: {} }));
    opening.current = undefined;
  }, [route]);
  // Escape closes the popup on top, as the Mendix client's does.
  useEffect(() => {
    if (!popups.length) return;
    const pressed = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setPopups((all) => all.slice(0, -1));
    };
    addEventListener('keydown', pressed);
    return () => removeEventListener('keydown', pressed);
  }, [popups.length]);
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
  // What every page is given by the application, the page itself and how
  // it is closed being its own.
  const shell = (current: string, objects: Record<string, DataObject>, close: () => void) => ({
    items: profile?.items || [],
    current,
    open,
    close,
    given: objects,
    changes,
    changed: () => {
      setProblem(undefined);
      setChanges((count) => count + 1);
    },
    fail: (error: unknown) => setProblem(error instanceof Error ? error.message : String(error)),
    notify: (message: string, level: string) => setNotice({ message, level }),
    form: findForm,
    collections,
    model: manifest,
    sidebar,
    setSidebar,
  });

  // The popups open over the page, each its own page in the shell.
  const windows = popups.map((popup, index) => {
    const form = findForm(popup.page);
    const shape = openingOf(popup.page);
    if (!form || !shape.popup) return null;
    // Closing a popup closes the ones opened from it too.
    const close = () => setPopups((all) => all.slice(0, index));
    return (
      <Fragment key={`${index}:${popup.page}`}>
        {shape.modal ? <div className="mx-underlay" style={{ zIndex: 1000 + index * 2 }} /> : null}
        <div
          role="dialog"
          aria-modal={shape.modal}
          aria-label={shape.title || undefined}
          className="modal-dialog mx-window mx-window-active mxrs-popup"
          data-page={popup.page}
          style={{
            zIndex: 1001 + index * 2,
            width: shape.width ? `${shape.width}px` : undefined,
            height: shape.height ? `${shape.height}px` : undefined,
          }}
        >
          <div className="modal-content mx-window-content">
            <div className="modal-header mx-window-header">
              <button
                type="button"
                className="close mx-window-close"
                aria-label="Close"
                onClick={close}
              >
                ×
              </button>
              <h4>{shape.title}</h4>
            </div>
            <div className="modal-body mx-window-body">
              <Shell.Provider value={shell(popup.page, popup.given, close)}>
                {form.document}
              </Shell.Provider>
            </div>
          </div>
        </div>
      </Fragment>
    );
  });

  // A page is what a route opens; a layout or a snippet is drawn inside one.
  if (declared?.kind === 'page') {
    return (
      <Shell.Provider value={shell(qualified, given[qualified] || {}, leave)}>
        {problem ? (
          <div role="alert" className="mxrs-failure" onClick={() => setProblem(undefined)}>
            {problem}
          </div>
        ) : null}
        {notice ? (
          <div
            role="status"
            className={`mxrs-notice mxrs-notice-${notice.level}`}
            onClick={() => setNotice(undefined)}
          >
            {notice.message}
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
        {windows}
      </Shell.Provider>
    );
  }

  return (
    <div className="mxrs-app">
      {windows}
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
