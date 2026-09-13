import { StrictMode, useEffect, useMemo, useState, type CSSProperties, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import './style.css';

type RuntimeValue = string | number | boolean | null | RuntimeValue[] | Record<string, unknown>;
type EventDecl = { event?: string; kind?: string; handler?: string; arguments?: Record<string, RuntimeValue> };
type Widget = {
  type: string;
  name?: string;
  options?: Record<string, RuntimeValue>;
  events?: EventDecl[];
  children?: Widget[];
};
type Page = { name: string; qualified_name: string; title?: string; widgets: Widget[] };
type NavigationItem = { caption?: string; page?: string; microflow?: string; items?: NavigationItem[] };
type Manifest = {
  format: number;
  project: { name: string; mendix_version: string };
  modules: Array<{ name: string; pages: Page[] }>;
  navigation: { profiles: Array<{ name: string; home_page?: string; items: NavigationItem[] }> };
};

const options = (widget: Widget) => widget.options || {};
const text = (value: RuntimeValue | undefined) => (value == null ? '' : String(value));
const caption = (widget: Widget) => text(options(widget).caption || widget.name || widget.type);
const children = (widget: Widget, context: Record<string, RuntimeValue> | null) =>
  (widget.children || []).map((child, index) => (
    <WidgetView key={`${child.name || child.type}-${index}`} widget={child} context={context} />
  ));
const style = (widget: Widget): CSSProperties | undefined => {
  const source = options(widget).style;
  if (typeof source !== 'string' || !source.trim()) return undefined;
  return Object.fromEntries(
    source.split(';').flatMap((entry) => {
      const [property, value] = entry.split(':', 2).map((part) => part.trim());
      if (!property || !value) return [];
      return [[property.replace(/-([a-z])/g, (_match, letter: string) => letter.toUpperCase()), value]];
    }),
  );
};

async function invoke(event: EventDecl | undefined, context: Record<string, RuntimeValue> | null) {
  if (!event?.handler) return;
  window.dispatchEvent(new CustomEvent('mxrs:action', { detail: { ...event, context } }));
  const response = await fetch(`./api/${encodeURIComponent(event.kind || 'action')}/${encodeURIComponent(event.handler)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ ...(event.arguments || {}), context }),
  });
  if (!response.ok) throw new Error(`Action ${event.handler} failed (${response.status})`);
}

function BoundInput({ widget, context }: { widget: Widget; context: Record<string, RuntimeValue> | null }) {
  const member = text(options(widget).attribute).split(/[./]/).at(-1) || widget.name || '';
  const [value, setValue] = useState(() => text(context?.[member]));
  useEffect(() => setValue(text(context?.[member])), [context, member]);
  if (widget.type === 'check_box') {
    return <input type="checkbox" checked={Boolean(context?.[member])} onChange={() => undefined} />;
  }
  if (widget.type === 'drop_down' || widget.type === 'combo_box') return <select value={value} onChange={(event) => setValue(event.target.value)}><option value="">—</option></select>;
  return <input type={widget.type === 'date_picker' ? 'date' : 'text'} value={value} onChange={(event) => setValue(event.target.value)} />;
}

function WidgetView({ widget, context }: { widget: Widget; context: Record<string, RuntimeValue> | null }) {
  const className = text(options(widget).class) || undefined;
  const content = children(widget, context);
  switch (widget.type) {
    case 'container': return <div className={className} style={style(widget)}>{content}</div>;
    case 'text': return <span className={className}>{caption(widget)}</span>;
    case 'button': return <button className={className} type="button" onClick={() => invoke(widget.events?.[0], context).catch(console.error)}>{caption(widget)}</button>;
    case 'text_box': case 'check_box': case 'date_picker': case 'drop_down': case 'combo_box': return <BoundInput widget={widget} context={context} />;
    case 'data_view': return <section className={className} data-widget="data-view">{content}</section>;
    case 'layout_grid': return <div className="mxrs-layout-grid">{content}</div>;
    case 'layout_grid_row': return <div className="mxrs-layout-row">{content}</div>;
    case 'layout_grid_column': {
      const weight = Number(options(widget).desktop || 1);
      return <div className="mxrs-layout-column" style={{ flexGrow: weight }}>{content}</div>;
    }
    case 'data_grid': case 'data_grid_2': return <section className="mxrs-data-grid"><strong>{caption(widget)}</strong><table><tbody><tr><td>No runtime records loaded</td></tr></tbody></table></section>;
    case 'gallery': return <section className="mxrs-gallery">{content.length ? content : 'No runtime records loaded'}</section>;
    case 'pluggable': return <section className={className} data-widget-id={text(options(widget).widget_id)}>{content.length ? content : caption(widget)}</section>;
    default: return <section className="mxrs-unsupported" data-widget-type={widget.type}>{caption(widget)}{content}</section>;
  }
}

function flattenPages(manifest: Manifest) {
  return manifest.modules.flatMap((module) => module.pages);
}

function navigationItems(items: NavigationItem[], open: (page: string) => void): ReactNode {
  return items.map((item, index) => (
    <li key={`${item.page || item.microflow || item.caption}-${index}`}>
      <button type="button" onClick={() => item.page && open(item.page)}>{item.caption || item.page || item.microflow}</button>
      {item.items?.length ? <ul>{navigationItems(item.items, open)}</ul> : null}
    </li>
  ));
}

function Application() {
  const [manifest, setManifest] = useState<Manifest | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [route, setRoute] = useState(() => decodeURIComponent(location.hash.slice(1)));
  useEffect(() => { fetch('./model.json').then((response) => {
    if (!response.ok) throw new Error(`model.json failed (${response.status})`);
    return response.json() as Promise<Manifest>;
  }).then(setManifest).catch((error: unknown) => setFailure(error instanceof Error ? error.message : String(error))); }, []);
  useEffect(() => { const changed = () => setRoute(decodeURIComponent(location.hash.slice(1))); addEventListener('hashchange', changed); return () => removeEventListener('hashchange', changed); }, []);
  const pages = useMemo(() => manifest ? flattenPages(manifest) : [], [manifest]);
  if (failure) return <main role="alert">Unable to load application: {failure}</main>;
  if (!manifest) return <main aria-busy="true">Loading…</main>;
  const profile = manifest.navigation.profiles[0];
  const requested = route || profile?.home_page || pages[0]?.qualified_name || '';
  const page = pages.find((candidate) => candidate.qualified_name === requested || candidate.name === requested);
  const open = (name: string) => { location.hash = encodeURIComponent(name); };
  return <div className="mxrs-app">
    <header><h1>{manifest.project.name}</h1><small>Mendix {manifest.project.mendix_version}</small></header>
    <div className="mxrs-shell">
      <nav aria-label="Application"><ul>{navigationItems(profile?.items || [], open)}</ul></nav>
      <main data-page={page?.qualified_name}>{page ? <><h2>{page.title || page.name}</h2>{page.widgets.map((widget, index) => <WidgetView key={`${widget.name || widget.type}-${index}`} widget={widget} context={null} />)}</> : <p role="alert">Page not found: {requested}</p>}</main>
    </div>
  </div>;
}

createRoot(document.getElementById('root')!).render(<StrictMode><Application /></StrictMode>);
