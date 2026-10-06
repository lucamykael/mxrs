// How each element of the page vocabulary is drawn. An element is the
// document the model stores; what the browser shows for it is decided here,
// by its stored type. A type nobody draws yet is a box holding its children.
import {
  Children,
  Fragment,
  isValidElement,
  useContext,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from 'react';

import { invokeAction } from '@/api/actions';
import { data, type DataObject } from '@/api/data';

import {
  ColumnFilter,
  Draft,
  OfPage,
  PageTitle,
  Placeholders,
  Row,
  Shell,
  Sidebar,
  type FilterKind,
} from './context';
import {
  child,
  className,
  content,
  entityOf,
  lastName,
  list,
  plain,
  shown,
  sourceOf,
  styleOf,
  text,
  textOf,
  widgetId,
  type Source,
} from './view';

type Draw = (source: Source) => ReactElement | null;

/** The element a field states, drawn. */
const held = (source: Source, field: string): ReactNode => {
  const stated = source.props[field];
  return isValidElement(stated) ? stated : null;
};

/**
 * What happens when the user acts on a widget: the action its field holds.
 * A flow runs once the user agrees to what it asks first, and without
 * arguments — a page holds no data to give it yet.
 */
function useAction(source: Source, field: string): (() => void) | undefined {
  const { open, changed, fail } = useContext(Shell);
  const row = useContext(Row);
  const draft = useContext(Draft);
  // One request at a time: a second click while the first is on its way
  // does nothing.
  const busy = useRef(false);
  const once = (work: () => Promise<unknown>) => () => {
    if (busy.current) return;
    busy.current = true;
    work()
      .catch(fail)
      .finally(() => {
        busy.current = false;
      });
  };
  const action = child(source, field);
  if (!action) return undefined;
  const settings = (name: string, target: string) => {
    const held = child(action, name);
    return held ? plain(held, target, '') : '';
  };
  const run = (kind: string, handler: string, asking: Source | null) => () => {
    const confirmation = asking ? child(asking, 'confirmationInfo') : null;
    const question = confirmation ? text(confirmation, 'question') : '';
    if (question && !window.confirm(question)) return;
    invokeAction({ kind, handler }, null).catch(fail);
  };
  // The object the widget is in: the one a form is filling, or a list's row.
  const here = draft?.object ?? row;
  switch (action.type) {
    case 'Forms$FormAction': {
      const page = settings('formSettings', 'form');
      if (!page) return undefined;
      // Each parameter the page is given takes the object the button is in.
      const pageSettings = child(action, 'formSettings');
      const given: Record<string, DataObject> = {};
      for (const mapping of pageSettings ? list(pageSettings, 'parameterMappings') : []) {
        const stated = sourceOf(mapping);
        if (stated && here) given[lastName(plain(stated, 'parameter', ''))] = here;
      }
      return () => open(page, given);
    }
    case 'Forms$CreateObjectClientAction': {
      const entity = entityOf(action);
      const page = settings('pageSettings', 'form');
      if (!entity || !page) return undefined;
      return once(() =>
        data<DataObject>('create', { entity }).then((created) =>
          open(page, { [lastName(entity)]: created }),
        ),
      );
    }
    case 'Forms$SaveChangesClientAction': {
      if (!draft?.object) return undefined;
      const { entity, id, members } = draft.object;
      // What the user changed is what is saved: the rest is not this
      // form's to overwrite.
      const sent = Object.fromEntries(
        Object.entries(members).filter(([member]) => draft.changed.has(member)),
      );
      return once(() =>
        data<DataObject>('save', {
          entity,
          id,
          new: draft.object?.new === true,
          members: sent,
        }).then((saved) => {
          draft.saved(saved);
          changed();
          if (plain(action, 'closePage', true)) history.back();
        }),
      );
    }
    case 'Forms$DeleteClientAction': {
      if (!here || here.new) return undefined;
      const { entity, id } = here;
      const confirm = once(() =>
        data('delete', { entity, id }).then(() => {
          changed();
          if (draft?.object && plain(action, 'closePage', true)) history.back();
        }),
      );
      // Deleting asks first, as the Mendix client does.
      return () => {
        if (window.confirm('Delete this item?')) confirm();
      };
    }
    case 'Forms$MicroflowAction': {
      const microflow = settings('microflowSettings', 'microflow');
      return microflow
        ? run('microflow', microflow, child(action, 'microflowSettings'))
        : undefined;
    }
    case 'Forms$CallNanoflowClientAction': {
      const nanoflow = plain(action, 'nanoflow', '');
      return nanoflow ? run('nanoflow', nanoflow, action) : undefined;
    }
    case 'Forms$ClosePageClientAction':
    case 'Forms$CancelChangesClientAction':
      return () => history.back();
    default:
      return undefined;
  }
}

/**
 * An icon a field holds: one of a collection — the classes the Mendix
 * client gives it, which the collection's stylesheet draws — a glyph, or
 * an image of a collection.
 */
const Icon = ({ source, field }: { source: Source; field: string }): ReactElement | null => {
  const { collections } = useContext(Shell);
  const held = child(source, field);
  if (!held) return null;
  switch (held.type) {
    case 'Forms$IconCollectionIcon': {
      const image = plain(held, 'image', '');
      const dot = image.lastIndexOf('.');
      const name = image.slice(dot + 1);
      if (!name) return null;
      const collection = collections.icons[image.slice(0, dot)];
      const classes = collection ? `${collection.class} ${collection.prefix}-${name}` : 'mx-icon';
      return <span className={classes} data-icon={image} aria-hidden="true" />;
    }
    case 'Forms$GlyphIcon': {
      const code = plain(held, 'code', 0);
      return code ? (
        <span className="glyphicon" aria-hidden="true">
          {String.fromCharCode(code)}
        </span>
      ) : null;
    }
    case 'Forms$ImageIcon': {
      const file = collections.images[plain(held, 'image', '')];
      return file ? <img className="mx-icon-image" src={`./${file}`} alt="" /> : null;
    }
    default:
      return null;
  }
};

const Box: Draw = (source) => (
  <div data-element={source.type} className={className(source)}>
    {content(source)}
  </div>
);

const Contents: Draw = (source) => <>{content(source)}</>;

const Page: Draw = (source) => (
  <PageTitle.Provider value={text(source, 'title')}>
    <OfPage.Provider value>{held(source, 'formCall')}</OfPage.Provider>
  </PageTitle.Provider>
);

const Layout: Draw = (source) => (
  <div className={className(source, 'mx-layout')}>{held(source, 'content')}</div>
);

/** A page inside its layout: each argument fills the placeholder it names. */
const LayoutCall: Draw = (source) => {
  // What an argument holds is the caller's: a placeholder in it is one of
  // the layout the caller is itself drawn in.
  const outer = useContext(Placeholders);
  const ofPage = useContext(OfPage);
  const filled = new Map<string, ReactNode>();
  for (const argument of Children.toArray(content(source))) {
    const stated = sourceOf(argument);
    if (!stated) continue;
    filled.set(
      lastName(plain(stated, 'parameter', '')),
      <Placeholders.Provider value={outer}>
        {/* A page's content is what the Mendix client calls `mx-page`. */}
        {ofPage ? <div className="mx-page">{content(stated)}</div> : content(stated)}
      </Placeholders.Provider>,
    );
  }
  const layout = useContext(Shell).form(plain(source, 'form', ''));
  if (!layout) {
    return (
      <>
        {[...filled].map(([name, held]) => (
          <Fragment key={name}>{held}</Fragment>
        ))}
      </>
    );
  }
  return (
    <OfPage.Provider value={false}>
      <Placeholders.Provider value={filled}>{layout.document}</Placeholders.Provider>
    </OfPage.Provider>
  );
};

/** A layout's content: the layout it is based on, when it is, and its own widgets. */
const LayoutContent: Draw = (source) => (
  <>
    {held(source, 'layoutCall')}
    {content(source)}
  </>
);

const Placeholder: Draw = (source) => (
  <>{useContext(Placeholders).get(plain(source, 'name', ''))}</>
);

const SnippetCallWidget: Draw = (source) => {
  const { form } = useContext(Shell);
  const call = child(source, 'formCall');
  const snippet = call ? form(plain(call, 'form', '')) : undefined;
  return <div className={className(source)}>{snippet?.document}</div>;
};

/** How wide or tall a region of a scroll container is, as its own settings say. */
function regionSize(region: Source, side: 'width' | 'height') {
  const size = plain(region, 'size', 0);
  const unit = { Pixels: 'px', Percentage: '%' }[plain(region, 'sizeMode', 'Auto')];
  return unit && size > 0 ? { [side]: `${size}${unit}`, flexBasis: `${size}${unit}` } : undefined;
}

/**
 * A scroll container, in the structure the Mendix client gives it — which
 * is what a theme styles: the top and bottom regions around a container
 * of its own, laid out the other way, for the left, center and right ones.
 */
const ScrollContainer: Draw = (source) => {
  // A side region that toggles: how it does, and whether it starts open.
  const toggling = (['left', 'right'] as const)
    .map((side) => child(source, side))
    .map((side) => (side ? plain(side, 'toggleMode', 'None') : 'None'))
    .find((mode) => mode !== 'None');
  // Open as its mode says until the user says otherwise, which holds from page to page.
  const { sidebar: chosen, setSidebar } = useContext(Shell);
  const open = chosen ?? !toggling?.endsWith('InitiallyClosed');
  const how = toggling?.startsWith('Push')
    ? 'push'
    : toggling?.startsWith('Slide')
      ? 'slide'
      : 'shrink';

  const region = (name: string, place: string, side: 'width' | 'height') => {
    const stated = child(source, name);
    if (!stated || !isValidElement(source.props[name])) return null;
    const toggles = plain(stated, 'toggleMode', 'None') !== 'None';
    return (
      <div
        className={className(
          stated,
          `mx-scrollcontainer-${place}`,
          toggles ? 'mx-scrollcontainer-toggleable' : '',
        )}
        // A region that toggles is as wide as its container says it is now.
        style={toggles ? undefined : regionSize(stated, side)}
      >
        <div
          className="mx-scrollcontainer-wrapper"
          style={toggles ? regionSize(stated, side) : undefined}
        >
          {content(stated)}
        </div>
      </div>
    );
  };
  const sidebar = (['left', 'right'] as const)
    .map((side) => child(source, side))
    .find((side) => side && plain(side, 'toggleMode', 'None') !== 'None');
  const size = sidebar ? regionSize(sidebar, 'width')?.width : undefined;
  return (
    <Sidebar.Provider value={{ open, toggle: () => setSidebar(!open) }}>
      <div
        className={className(
          source,
          'mx-scrollcontainer',
          'mx-scrollcontainer-vertical',
          'mx-scrollcontainer-fixed',
        )}
      >
        {region('top', 'top', 'height')}
        <div
          className={[
            'mx-scrollcontainer mx-scrollcontainer-horizontal mx-scrollcontainer-nested mx-scrollcontainer-fixed',
            toggling ? `mx-scrollcontainer-${how}` : '',
            toggling && open ? 'mx-scrollcontainer-open' : '',
          ]
            .filter(Boolean)
            .join(' ')}
          style={size ? ({ '--sidebar-size': size } as CSSProperties) : undefined}
        >
          {region('left', 'left', 'width')}
          {region('centerRegion', 'center', 'width')}
          {region('right', 'right', 'width')}
        </div>
        {region('bottom', 'bottom', 'height')}
      </div>
    </Sidebar.Provider>
  );
};

const Region: Draw = (source) => <div className={className(source)}>{content(source)}</div>;

const DivContainer: Draw = (source) => {
  const act = useAction(source, 'onClickAction');
  return (
    <div
      className={className(source, 'mx-container')}
      style={styleOf(source)}
      onClick={act}
      role={act ? 'button' : undefined}
    >
      {content(source)}
    </div>
  );
};

const LayoutGrid: Draw = (source) => {
  const width =
    plain(source, 'width', 'FullWidth') === 'FullWidth'
      ? ['mx-layoutgrid-fluid', 'container-fluid']
      : ['mx-layoutgrid-fixed', 'container'];
  return (
    <div className={className(source, 'mx-layoutgrid', ...width)} style={styleOf(source)}>
      {content(source)}
    </div>
  );
};

const LayoutGridRow: Draw = (source) => (
  <div className={className(source, 'row')}>{content(source)}</div>
);

/** A column, as wide on each kind of screen as its weights say: the grid classes a theme styles. */
const LayoutGridColumn: Draw = (source) => {
  const sized = (prefix: string, field: string) => {
    const weight = plain(source, field, -1);
    return weight > 0 ? `${prefix}-${weight}` : prefix;
  };
  return (
    <div
      className={className(
        source,
        sized('col-lg', 'weight'),
        sized('col-md', 'tabletWeight'),
        sized('col', 'phoneWeight'),
      )}
    >
      {content(source)}
    </div>
  );
};

const DynamicText: Draw = (source) => {
  const mode = plain(source, 'renderMode', 'Text');
  const Tag = (/^H[1-6]$/.test(mode) ? mode.toLowerCase() : mode === 'Paragraph' ? 'p' : 'span') as
    'h1' | 'p' | 'span';
  const row = useContext(Row);
  const object = useContext(Draft)?.object ?? row;
  return (
    <Tag className={className(source, 'mx-text')} style={styleOf(source)}>
      {text(source, 'content', object)}
    </Tag>
  );
};

const StaticText: Draw = (source) => (
  <span className={className(source, 'mx-text')} style={styleOf(source)}>
    {text(source, 'caption')}
  </span>
);

const Label: Draw = (source) => (
  <label className={className(source, 'control-label')}>{text(source, 'caption')}</label>
);

const ActionButton: Draw = (source) => {
  const act = useAction(source, 'action');
  const style = plain(source, 'buttonStyle', 'Default').toLowerCase();
  return (
    <button
      type="button"
      className={className(source, 'btn', 'mx-button', `btn-${style}`)}
      style={styleOf(source)}
      title={text(source, 'tooltip') || undefined}
      onClick={act}
    >
      <Icon source={source} field="icon" />
      {text(source, 'captionTemplate')}
    </button>
  );
};

/** What a control of a form is given: the attribute it fills, its value and how to change it. */
type Bound = {
  name: string;
  placeholder: string;
  value: unknown;
  change: (value: unknown) => void;
};

/** A widget bound to an attribute: its label, and a control of its kind holding the form's value. */
const input =
  (control: (bound: Bound) => ReactElement): Draw =>
  (source) => {
    const draft = useContext(Draft);
    const attribute = child(source, 'attributeRef');
    const name = lastName(attribute ? plain(attribute, 'attribute', '') : '');
    const label = text(source, 'labelTemplate');
    return (
      <div className={className(source, 'form-group')}>
        {label ? <label className="control-label">{label}</label> : null}
        {control({
          name,
          placeholder: text(source, 'placeholderTemplate'),
          value: draft?.object?.members[name],
          change: (value) => draft?.set(name, value),
        })}
      </div>
    );
  };

const written = (value: unknown) => (value === null || value === undefined ? '' : String(value));

const TextBox = input(({ name, placeholder, value, change }) => (
  <input
    className="form-control"
    name={name}
    placeholder={placeholder}
    value={written(value)}
    onChange={(event) => change(event.target.value)}
  />
));
const TextArea = input(({ name, placeholder, value, change }) => (
  <textarea
    className="form-control"
    name={name}
    placeholder={placeholder}
    value={written(value)}
    onChange={(event) => change(event.target.value)}
  />
));
const DatePicker = input(({ name, value, change }) => (
  <input
    className="form-control"
    type="date"
    name={name}
    value={written(value).slice(0, 10)}
    onChange={(event) => change(event.target.value || null)}
  />
));
const CheckBox = input(({ name, value, change }) => (
  <input
    type="checkbox"
    name={name}
    checked={value === true}
    onChange={(event) => change(event.target.checked)}
  />
));
const Selector = input(({ name }) => <select className="form-control" name={name} />);

/**
 * A data view: the object its page was given, which its inputs fill and
 * its buttons save. One whose source is a flow shows its widgets without
 * an object, as a page does before its data arrives.
 */
const DataView: Draw = (source) => {
  const { given } = useContext(Shell);
  const origin = child(source, 'dataSource');
  const variable = origin ? child(origin, 'sourceVariable') : null;
  const parameter = variable ? lastName(plain(variable, 'pageParameter', '')) : '';
  const initial = parameter ? (given[parameter] ?? null) : null;
  const [object, setObject] = useState<DataObject | null>(initial);
  const [changed, setChanged] = useState<ReadonlySet<string>>(new Set());
  const set = (member: string, value: unknown) => {
    setObject((now) => (now ? { ...now, members: { ...now.members, [member]: value } } : now));
    setChanged((now) => new Set(now).add(member));
  };
  const saved = (kept: DataObject) => {
    setObject(kept);
    setChanged(new Set());
  };
  // A page opened without the object it shows — from the address bar, or
  // reloaded — has nothing to show, and says so.
  if (parameter && !object) {
    return (
      <section className={className(source, 'mx-dataview')}>
        <div className="mx-dataview-empty">
          {text(source, 'noEntityMessage') ||
            'Nothing to show: open this page from the list it belongs to.'}
        </div>
      </section>
    );
  }
  return (
    <Draft.Provider value={{ object, changed, set, saved }}>
      <section className={className(source, 'mx-dataview')}>
        <div className="mx-dataview-content">{each(list(source, 'widgets'))}</div>
        {list(source, 'footerWidgets').length ? (
          <footer className="mx-dataview-controls">{each(list(source, 'footerWidgets'))}</footer>
        ) : null}
      </section>
    </Draft.Provider>
  );
};

/** A list: every object of its entity, each drawn as the widgets inside. */
const ListView: Draw = (source) => {
  const origin = child(source, 'dataSource');
  const entity = origin ? entityOf(origin) : '';
  const objects = useObjects(entity);
  // A list about no entity this page can read shows its widgets once.
  if (!entity) {
    return <section className={className(source, 'mx-listview')}>{content(source)}</section>;
  }
  return (
    <section className={className(source, 'mx-listview')} aria-busy={objects === undefined}>
      <ul>
        {(objects ?? []).map((object) => (
          <li key={object.id} className="mx-listview-item">
            {/* A row is about its own object, whatever form the list is in. */}
            <Row.Provider value={object}>
              <Draft.Provider value={null}>{content(source)}</Draft.Provider>
            </Row.Provider>
          </li>
        ))}
      </ul>
      {objects?.length === 0 ? <div className="mx-listview-empty">No items</div> : null}
    </section>
  );
};

const GroupBox: Draw = (source) => (
  <fieldset className={className(source, 'mx-groupbox')}>
    <legend>{text(source, 'captionTemplate') || text(source, 'caption')}</legend>
    {content(source)}
  </fieldset>
);

const TabControl: Draw = (source) => {
  const [active, setActive] = useState(0);
  const pages = Children.toArray(content(source)).map(sourceOf);
  const shown = pages[Math.min(active, pages.length - 1)];
  return (
    <div className={className(source, 'mx-tabcontainer')}>
      <div role="tablist" className="mx-tabcontainer-tabs">
        {pages.map((page, index) => (
          <button
            key={index}
            type="button"
            role="tab"
            aria-selected={pages[index] === shown}
            onClick={() => setActive(index)}
          >
            {page ? text(page, 'caption') || plain(page, 'name', '') : ''}
          </button>
        ))}
      </div>
      <div role="tabpanel">{shown ? content(shown) : null}</div>
    </div>
  );
};

/** The application's menu, in the structure the Mendix client gives a navigation tree. */
const Menu: Draw = (source) => {
  const { items, current, open } = useContext(Shell);
  const list = (entries: typeof items): ReactElement => (
    <ul>
      {entries.map((item, index) => (
        <li
          key={index}
          className={
            [
              item.page === current ? 'active' : '',
              item.items?.length ? 'mx-navigationtree-has-items' : '',
            ]
              .filter(Boolean)
              .join(' ') || undefined
          }
        >
          <a
            title={item.caption || undefined}
            className={item.page === current ? 'active' : undefined}
            href={item.page ? `#${encodeURIComponent(item.page)}` : undefined}
            onClick={(event) => {
              event.preventDefault();
              if (item.page) open(item.page);
            }}
          >
            {/* The icon is what a closed sidebar shows of the item. */}
            {typeof item.icon === 'string' && item.icon ? (
              <span className={`glyphicon glyphicon-${item.icon}`} aria-hidden="true" />
            ) : null}
            {item.caption || item.page || item.microflow}
          </a>
          {item.items?.length ? list(item.items) : null}
        </li>
      ))}
    </ul>
  );
  return (
    <div className={className(source, 'mx-navigationtree')}>
      <div className="navbar-inner">{list(items)}</div>
    </div>
  );
};

/** Opens and closes the sidebar of the layout it is in. */
const SidebarToggle: Draw = (source) => {
  const { open, toggle } = useContext(Sidebar);
  const style = plain(source, 'buttonStyle', 'Default').toLowerCase();
  return (
    <button
      type="button"
      className={className(source, 'btn', 'mx-button', `btn-${style}`, 'mx-sidebartoggle')}
      title={text(source, 'tooltip')}
      aria-expanded={open}
      onClick={toggle}
    >
      <Icon source={source} field="icon" />
      {text(source, 'captionTemplate')}
    </button>
  );
};

/** A list of elements a field states, each drawn. */
const each = (value: unknown): ReactNode =>
  Array.isArray(value)
    ? value.map((item, index) => <Fragment key={index}>{item as ReactNode}</Fragment>)
    : null;

const Header: Draw = (source) => (
  <header className={className(source, 'mx-header')}>
    <div>{each(list(source, 'leftWidgets'))}</div>
    <div>{each(list(source, 'rightWidgets'))}</div>
  </header>
);

/** The title of the page being drawn. */
const Title: Draw = (source) => (
  <h1 className={className(source, 'mx-title')}>{useContext(PageTitle)}</h1>
);

/** A button of a grid's control bar: its caption. What it does needs the grid's data. */
const GridButton: Draw = (source) => (
  <button
    type="button"
    className={className(
      source,
      'btn',
      'mx-button',
      `btn-${plain(source, 'buttonStyle', 'Default').toLowerCase() || 'default'}`,
    )}
  >
    {text(source, 'captionTemplate')}
  </button>
);

/** A static image: the image of a collection it names, when the build wrote it. */
const Image: Draw = (source) => {
  const { collections } = useContext(Shell);
  const image = plain(source, 'image', '');
  const file = collections.images[image];
  if (file) {
    return (
      <img className={className(source, 'mx-image')} src={`./${file}`} alt={lastName(image)} />
    );
  }
  return <span role="img" className={className(source, 'mx-image')} aria-label={lastName(image)} />;
};

/** A table: every cell where its row and column say, as wide and tall as it spans. */
const Table: Draw = (source) => (
  <div className={className(source, 'mx-table')}>
    {list(source, 'cells').map((cell, index) => {
      const stated = sourceOf(cell);
      if (!stated) return null;
      const place = (start: string, span: string) =>
        `${plain(stated, start, 0) + 1} / span ${plain(stated, span, 1)}`;
      return (
        <div
          key={index}
          className={className(stated)}
          style={{
            gridColumn: place('leftColumnIndex', 'width'),
            gridRow: place('topRowIndex', 'height'),
          }}
        >
          {content(stated)}
        </div>
      );
    })}
  </div>
);

const NavigationItem: Draw = (source) => {
  const act = useAction(source, 'action');
  return (
    <div
      className={className(source, 'mx-navigationlist-item')}
      onClick={act}
      role={act ? 'button' : undefined}
    >
      {content(source)}
    </div>
  );
};

/** A grid of the model's own kind: its buttons and a heading per column. */
const DataGrid: Draw = (source) => (
  <section className={className(source, 'mx-datagrid')}>
    {held(source, 'controlBar')}
    <table>
      <thead>
        <tr>
          {list(source, 'columns').map((column, index) => {
            const stated = sourceOf(column);
            return <th key={index}>{stated ? text(stated, 'caption') : null}</th>;
          })}
        </tr>
      </thead>
    </table>
  </section>
);

/**
 * Every widget a pluggable widget's properties hold, wherever they are. A
 * widget is what has an appearance; what a property says besides — a data
 * source, an action, a template — is not drawn.
 */
function nested(value: unknown): ReactNode[] {
  const stated = sourceOf(value);
  if (stated) {
    const widget = 'appearance' in stated.defaults || stated.type === 'CustomWidgets$CustomWidget';
    return widget ? [value as ReactElement] : Object.values(stated.props).flatMap(nested);
  }
  if (isValidElement(value)) return [];
  if (Array.isArray(value)) return value.flatMap(nested);
  if (typeof value === 'object' && value !== null) return Object.values(value).flatMap(nested);
  return [];
}

/** What a page states of a pluggable widget's property, by its key. */
const property = (source: Source, key: string): unknown =>
  (source.props.properties as Record<string, unknown> | undefined)?.[key];

/**
 * Mendix's Image widget: an image of a collection, an image at a URL, or
 * an icon — in the markup its own stylesheet styles.
 */
const ImageWidget: Draw = (source) => {
  const { collections } = useContext(Shell);
  const kind = property(source, 'datasource');
  const responsive = property(source, 'responsive') !== false;
  const classes = className(
    source,
    'mx-image-viewer',
    responsive ? 'mx-image-viewer-responsive' : '',
  );
  const object = property(source, 'imageObject');
  const file = typeof object === 'string' ? collections.images[object] : undefined;
  if (kind === 'icon') {
    const icon = sourceOf(property(source, 'imageIcon'));
    return (
      <div className={classes}>
        {icon ? (
          <Icon
            source={{ ...source, props: { icon: property(source, 'imageIcon') } }}
            field="icon"
          />
        ) : null}
      </div>
    );
  }
  const src = kind === 'imageUrl' ? textOf(property(source, 'imageUrl')) : file ? `./${file}` : '';
  return <div className={classes}>{src ? <img src={src} alt="" /> : null}</div>;
};

// --- data widgets: what a page lists, drawn as Mendix's own widgets draw it ---

/** A column of a grid, or an option of a filter: what the page states of it. */
type Stated = Record<string, unknown>;

const statedList = (value: unknown): Stated[] =>
  Array.isArray(value)
    ? value.filter(
        (item): item is Stated =>
          typeof item === 'object' && item !== null && !isValidElement(item),
      )
    : [];

/** The entity a pluggable widget's data source lists, when it lists one the runtime can read. */
function sourceEntity(value: unknown): string {
  const stated = sourceOf(value);
  if (!stated) return '';
  switch (stated.type) {
    case 'CustomWidgets$CustomWidgetXPathSource':
    case 'CustomWidgets$CustomWidgetDatabaseSource':
      return entityOf(stated);
    default:
      // A microflow, an association, a listened widget: nothing a page can
      // ask the runtime for yet.
      return '';
  }
}

/**
 * The objects of an entity, read again whenever the data changes;
 * `undefined` while they are on their way, and none for no entity.
 */
function useObjects(entity: string): DataObject[] | undefined {
  const { changes, fail } = useContext(Shell);
  const [objects, setObjects] = useState<DataObject[]>();
  useEffect(() => {
    if (!entity) return;
    let current = true;
    data<{ objects: DataObject[] }>('retrieve', { entity })
      .then((answer) => current && setObjects(answer.objects))
      .catch((error) => current && (setObjects([]), fail(error)));
    return () => {
      current = false;
    };
    // `fail` is the application's own and does not change what is listed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entity, changes]);
  return entity ? objects : undefined;
}

/** The text a pluggable widget's property says: a text, a template, or a widget value holding one. */
function widgetText(source: Source, key: string, object?: DataObject | null): string {
  const value = property(source, key);
  const stated = sourceOf(value);
  if (!stated) return textOf(value);
  if (stated.type === 'CustomWidgets$WidgetValue') return text(stated, 'textTemplate', object);
  return text({ ...source, props: { held: value } }, 'held', object);
}

/** The value of an object's attribute a pluggable widget's property names. */
const memberOf = (object: DataObject | null, attribute: unknown): unknown =>
  object && typeof attribute === 'string' && attribute
    ? object.members[lastName(attribute)]
    : undefined;

/** What a filter holds: what the user chose, and how it narrows. */
type Filter = { value: string; kind: FilterKind };

/** Whether a value passes a filter: contains the text, equals the option, or falls on the day. */
function matches(value: unknown, filter: Filter): boolean {
  switch (filter.kind) {
    case 'option':
      return String(value) === filter.value || shown(value) === filter.value;
    case 'date':
      return typeof value === 'string' && value.startsWith(filter.value);
    default:
      return shown(value).toLowerCase().includes(filter.value.toLowerCase());
  }
}

/** Whether a row passes what the user chose in a column's filter. */
function passes(column: Stated, object: DataObject, filter: Filter | undefined): boolean {
  if (!filter?.value) return true;
  const value = memberOf(object, column.attribute);
  return value === undefined || matches(value, filter);
}

/** Whether a row passes the grid's own filter: any attribute column of it does. */
function passesAny(columns: Stated[], object: DataObject, filter: Filter | undefined): boolean {
  if (!filter?.value) return true;
  const values = columns
    .map((column) => memberOf(object, column.attribute))
    .filter((value) => value !== undefined);
  return values.length === 0 || values.some((value) => matches(value, filter));
}

/** The pages of a list, as the widget's pagination says: all of it, or so many at a time. */
function usePaging(count: number, source: Source) {
  const [page, setPage] = useState(0);
  const size = Math.max(1, Number(property(source, 'pageSize') ?? 10) || 10);
  const paged = property(source, 'pagination') !== 'virtualScrolling';
  const pages = Math.max(1, Math.ceil(count / size));
  const current = paged ? Math.min(page, pages - 1) : 0;
  const first = current * size;
  return {
    paged,
    first,
    last: paged ? Math.min(first + size, count) : count,
    count,
    back: () => setPage(Math.max(0, current - 1)),
    forward: () => setPage(Math.min(pages - 1, current + 1)),
    reset: () => setPage(0),
  };
}

/** The bar of a paged list: where it is, and the way back and forward. */
const PagingBar = ({ paging }: { paging: ReturnType<typeof usePaging> }) => (
  <div className="pagination-bar">
    <button
      type="button"
      className="btn pagination-button"
      disabled={paging.first === 0}
      onClick={paging.back}
      aria-label="Previous page"
    >
      ‹
    </button>
    <span className="paging-status">
      {paging.count ? `${paging.first + 1} to ${paging.last} of ${paging.count}` : '0 to 0 of 0'}
    </span>
    <button
      type="button"
      className="btn pagination-button"
      disabled={paging.last >= paging.count}
      onClick={paging.forward}
      aria-label="Next page"
    >
      ›
    </button>
  </div>
);

/** What a list's rows are drawn about: each its own object, and no form's. */
const OfRow = ({ object, children }: { object: DataObject; children: ReactNode }) => (
  <Row.Provider value={object}>
    <Draft.Provider value={null}>{children}</Draft.Provider>
  </Row.Provider>
);

/** The widgets a pluggable widget's property holds, drawn. */
const inside = (value: unknown): ReactNode => (isValidElement(value) ? value : each(value));

/**
 * Mendix's Data grid 2: the objects of its entity, a row each, in the
 * columns the page states — an attribute's value, a text, or widgets of
 * the page's own — with each column's filter and the grid's pages.
 */
const Datagrid: Draw = (source) => {
  const entity = sourceEntity(property(source, 'datasource'));
  const objects = useObjects(entity);
  const columns = statedList(property(source, 'columns'));
  // Each column's filter by its index; the grid's own, above the columns, by -1.
  const [filters, setFilters] = useState<Record<number, Filter>>({});
  const rows = (objects ?? []).filter(
    (object) =>
      passesAny(columns, object, filters[-1]) &&
      columns.every((column, index) => passes(column, object, filters[index])),
  );
  const paging = usePaging(rows.length, source);
  const filter = (index: number) => ({
    value: filters[index]?.value ?? '',
    set: (value: string, kind: FilterKind) => {
      setFilters((now) => ({ ...now, [index]: { value, kind } }));
      paging.reset();
    },
  });
  const cell = (column: Stated, object: DataObject): ReactNode => {
    switch (column.showContentAs) {
      case 'customContent':
        return inside(column.content);
      case 'dynamicText':
        return text({ ...source, props: { held: column.dynamicText } }, 'held', object);
      default:
        return shown(memberOf(object, column.attribute));
    }
  };
  const width = (column: Stated) =>
    column.width === 'autoFit'
      ? 'fit-content(100%)'
      : column.width === 'manual' && typeof column.size === 'number'
        ? `${column.size}px`
        : 'minmax(100px, 1fr)';
  const filtersPlaceholder = property(source, 'filtersPlaceholder');
  return (
    <div className={className(source, 'widget-datagrid')}>
      {filtersPlaceholder ? (
        <div className="widget-datagrid-header header-filters">
          <ColumnFilter.Provider value={filter(-1)}>
            {inside(filtersPlaceholder)}
          </ColumnFilter.Provider>
        </div>
      ) : null}
      <div className="widget-datagrid-content">
        <div
          className="widget-datagrid-grid table"
          role="table"
          style={
            { '--widgets-grid-template-columns': columns.map(width).join(' ') } as CSSProperties
          }
        >
          <div className="widget-datagrid-grid-head" role="rowgroup">
            <div className="tr" role="row" style={{ display: 'contents' }}>
              {columns.map((column, index) => (
                <div className="th" role="columnheader" key={index}>
                  <div className="column-container">
                    <div className="column-header">{textOf(column.header)}</div>
                    {isValidElement(column.filter) ? (
                      <div className="filter">
                        <ColumnFilter.Provider value={filter(index)}>
                          {column.filter}
                        </ColumnFilter.Provider>
                      </div>
                    ) : null}
                  </div>
                </div>
              ))}
            </div>
          </div>
          <div
            className="widget-datagrid-grid-body"
            role="rowgroup"
            aria-busy={entity !== '' && objects === undefined}
          >
            {rows.slice(paging.first, paging.last).map((object) => (
              <div className="tr" role="row" key={object.id} style={{ display: 'contents' }}>
                <OfRow object={object}>
                  {columns.map((column, index) => (
                    <div
                      className={column.wrapText ? 'td wrap-text' : 'td'}
                      role="cell"
                      key={index}
                    >
                      {cell(column, object)}
                    </div>
                  ))}
                </OfRow>
              </div>
            ))}
          </div>
        </div>
        {entity &&
        objects?.length === 0 &&
        property(source, 'showEmptyPlaceholder') === 'custom' ? (
          <div className="widget-datagrid-empty">
            {inside(property(source, 'emptyPlaceholder'))}
          </div>
        ) : null}
      </div>
      {paging.paged && entity ? (
        <div className="widget-datagrid-footer">
          <PagingBar paging={paging} />
        </div>
      ) : null}
    </div>
  );
};

/**
 * Mendix's Gallery: the objects of its entity, each drawn as the widgets
 * inside, so many to a row as the page says for a desktop.
 */
const Gallery: Draw = (source) => {
  const entity = sourceEntity(property(source, 'datasource'));
  const objects = useObjects(entity);
  const [filter, setFilter] = useState<Filter>();
  const items = (objects ?? []).filter(
    (object) =>
      !filter?.value || Object.values(object.members).some((value) => matches(value, filter)),
  );
  const paging = usePaging(items.length, source);
  const across = (key: string, otherwise: number) =>
    Math.max(1, Number(property(source, key) ?? otherwise) || otherwise);
  const desktop = across('desktopItems', 4);
  const filtersPlaceholder = property(source, 'filtersPlaceholder');
  return (
    <div
      className={className(
        source,
        'widget-gallery',
        `widget-gallery-lg-${desktop}`,
        `widget-gallery-md-${across('tabletItems', 3)}`,
        `widget-gallery-sm-${across('phoneItems', 1)}`,
      )}
    >
      {filtersPlaceholder ? (
        <div className="widget-gallery-filter">
          <ColumnFilter.Provider
            value={{
              value: filter?.value ?? '',
              set: (value, kind) => {
                setFilter({ value, kind });
                paging.reset();
              },
            }}
          >
            {inside(filtersPlaceholder)}
          </ColumnFilter.Provider>
        </div>
      ) : null}
      <div className="widget-gallery-items" aria-busy={entity !== '' && objects === undefined}>
        {items.slice(paging.first, paging.last).map((object) => (
          <div className="widget-gallery-item" key={object.id}>
            <OfRow object={object}>{inside(property(source, 'content'))}</OfRow>
          </div>
        ))}
      </div>
      {entity && objects?.length === 0 && property(source, 'showEmptyPlaceholder') === 'custom' ? (
        <div className="widget-gallery-empty">{inside(property(source, 'emptyPlaceholder'))}</div>
      ) : null}
      {paging.paged && entity ? (
        <div className="widget-gallery-footer">
          <PagingBar paging={paging} />
        </div>
      ) : null}
    </div>
  );
};

/** A grid column's text filter: what the user types narrows the rows. */
const TextFilter: Draw = (source) => {
  const filter = useContext(ColumnFilter);
  return (
    <div className={className(source, 'filter-container')}>
      <input
        className="form-control filter-input"
        type="text"
        placeholder={widgetText(source, 'placeholder')}
        value={filter?.value ?? ''}
        onChange={(event) => filter?.set(event.target.value, 'text')}
      />
    </div>
  );
};

/** A grid column's drop-down filter: one of the options the page states, or any. */
const DropdownFilter: Draw = (source) => {
  const filter = useContext(ColumnFilter);
  const options = statedList(property(source, 'filterOptions'));
  return (
    <div className={className(source, 'filter-container')}>
      <select
        className="form-control filter-input"
        value={filter?.value ?? ''}
        onChange={(event) => filter?.set(event.target.value, 'option')}
      >
        <option value="">{widgetText(source, 'emptyOptionCaption')}</option>
        {options.map((option, index) => (
          <option key={index} value={textOf(option.value)}>
            {textOf(option.caption)}
          </option>
        ))}
      </select>
    </div>
  );
};

/** A grid column's date filter: the day typed narrows the rows to it. */
const DateFilter: Draw = (source) => {
  const filter = useContext(ColumnFilter);
  return (
    <div className={className(source, 'filter-container')}>
      <input
        className="form-control filter-input"
        type="date"
        value={filter?.value ?? ''}
        onChange={(event) => filter?.set(event.target.value, 'date')}
      />
    </div>
  );
};

/** Mendix's Combo box: the attribute of the form's object it is about, typed into. */
const Combobox: Draw = (source) => {
  const draft = useContext(Draft);
  const attribute = [
    'attributeEnumeration',
    'attributeBoolean',
    'attributeAssociation',
    'databaseAttributeString',
  ]
    .map((key) => property(source, key))
    .find((value): value is string => typeof value === 'string' && value !== '');
  const name = attribute ? lastName(attribute) : '';
  const value = name ? draft?.object?.members[name] : undefined;
  // In the group a form's control is in, which is what a theme styles.
  return (
    <div className={className(source, 'form-group')}>
      <div className="widget-combobox">
        <div className="form-control widget-combobox-input-container">
          <input
            className="widget-combobox-input"
            name={name || undefined}
            placeholder={widgetText(source, 'emptyOptionText')}
            value={written(value)}
            readOnly={!name || !draft}
            onChange={(event) => name && draft?.set(name, event.target.value)}
          />
        </div>
      </div>
    </div>
  );
};

/** Mendix's Badge: its value, as a badge or a label of its style. */
const Badge: Draw = (source) => {
  const row = useContext(Row);
  const object = useContext(Draft)?.object ?? row;
  const label = property(source, 'type') === 'label';
  const style =
    typeof property(source, 'bootstrapStyle') === 'string'
      ? (property(source, 'bootstrapStyle') as string)
      : 'default';
  return (
    <span
      className={className(
        source,
        'widget-badge',
        label ? 'label' : 'badge',
        `${label ? 'label' : 'badge'}-${style}`,
      )}
    >
      {widgetText(source, 'value', object)}
    </span>
  );
};

/** What a progress widget's property says its value is: a number, or the object's attribute. */
function progressValue(
  source: Source,
  name: string,
  object: DataObject | null,
  otherwise: number,
): number {
  const kind = property(source, 'type');
  const value =
    kind === 'dynamic'
      ? memberOf(object, property(source, `dynamic${name}`))
      : kind === 'expression'
        ? undefined
        : property(source, `static${name}`);
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) ? number : otherwise;
}

/** The share a progress widget shows, from its value between its minimum and maximum. */
function usePercentage(source: Source): number {
  const row = useContext(Row);
  const object = useContext(Draft)?.object ?? row;
  const value = progressValue(source, 'CurrentValue', object, 0);
  const minimum = progressValue(source, 'MinValue', object, 0);
  const maximum = progressValue(source, 'MaxValue', object, 100);
  const share = maximum > minimum ? ((value - minimum) / (maximum - minimum)) * 100 : 0;
  return Math.round(Math.max(0, Math.min(100, share)));
}

/** Mendix's Progress bar: how far along its value is. */
const ProgressBar: Draw = (source) => {
  const percentage = usePercentage(source);
  return (
    <div className={className(source, 'widget-progress-bar')}>
      <div className="progress">
        <div className="progress-bar" role="progressbar" style={{ width: `${percentage}%` }}>
          {property(source, 'showLabel') === false ? null : `${percentage}%`}
        </div>
      </div>
    </div>
  );
};

/** Mendix's Progress circle: the same, as a ring. */
const ProgressCircle: Draw = (source) => {
  const percentage = usePercentage(source);
  return (
    <div className={className(source, 'widget-progress-circle')}>
      <div
        className="progress-circle"
        role="progressbar"
        style={{ background: `conic-gradient(currentColor ${percentage}%, transparent 0)` }}
      >
        <span className="progress-circle-label">{`${percentage}%`}</span>
      </div>
    </div>
  );
};

/** A widget drawn as the box its own stylesheet styles: its classes, and what it holds. */
const boxed =
  (...classes: string[]): Draw =>
  (source) => (
    <div className={className(source, ...classes)}>{each(nested(source.props.properties))}</div>
  );

/** The pluggable widgets drawn as themselves, by the id their definition states. */
const customWidgets: Record<string, Draw> = {
  'com.mendix.widget.web.image.Image': ImageWidget,
  'com.mendix.widget.web.datagrid.Datagrid': Datagrid,
  'com.mendix.widget.web.gallery.Gallery': Gallery,
  'com.mendix.widget.web.datagridtextfilter.DatagridTextFilter': TextFilter,
  'com.mendix.widget.web.datagriddropdownfilter.DatagridDropdownFilter': DropdownFilter,
  'com.mendix.widget.web.datagriddatefilter.DatagridDateFilter': DateFilter,
  'com.mendix.widget.web.combobox.Combobox': Combobox,
  'com.mendix.widget.custom.badge.Badge': Badge,
  'com.mendix.widget.custom.progressbar.ProgressBar': ProgressBar,
  'com.mendix.widget.custom.progresscircle.ProgressCircle': ProgressCircle,
  'com.mendix.widget.web.timeline.Timeline': boxed('widget-timeline'),
  'com.mendix.widget.web.barchart.BarChart': boxed('widget-charts', 'widget-bar-chart'),
  'com.mendix.widget.web.columnchart.ColumnChart': boxed('widget-charts', 'widget-column-chart'),
  'com.mendix.widget.web.linechart.LineChart': boxed('widget-charts', 'widget-line-chart'),
  'com.mendix.widget.web.piechart.PieChart': boxed('widget-charts', 'widget-pie-chart'),
};

/** A pluggable widget: as itself when it is drawn; else its label, its name, and the widgets its properties hold. */
const CustomWidget: Draw = (source) => {
  const id = widgetId(source);
  const Drawn = id ? customWidgets[id] : undefined;
  if (Drawn) return <Drawn {...source} />;
  const definition = sourceOf(source.defaults.definition);
  const widget = definition ? plain(definition, 'widgetName', '') : '';
  return (
    <section className={className(source, 'mx-widget')} data-widget={widget}>
      {text(source, 'labelTemplate') ? (
        <label className="control-label">{text(source, 'labelTemplate')}</label>
      ) : null}
      {each(nested(source.props.properties))}
    </section>
  );
};

const drawn: Record<string, Draw> = {
  Forms$Page: Page,
  Forms$Layout: Layout,
  Forms$Snippet: Contents,
  Forms$LayoutCall: LayoutCall,
  Forms$FormCallArgument: Contents,
  Forms$WebLayoutContent: LayoutContent,
  Forms$NativeLayoutContent: Contents,
  Forms$Placeholder: Placeholder,
  Forms$SnippetCallWidget: SnippetCallWidget,
  Forms$ScrollContainer: ScrollContainer,
  Forms$ScrollContainerRegion: Region,
  Forms$DivContainer: DivContainer,
  Forms$LayoutGrid: LayoutGrid,
  Forms$LayoutGridRow: LayoutGridRow,
  Forms$LayoutGridColumn: LayoutGridColumn,
  Forms$DynamicText: DynamicText,
  Forms$Text: StaticText,
  Forms$Label: Label,
  Forms$ActionButton: ActionButton,
  Forms$TextBox: TextBox,
  Forms$TextArea: TextArea,
  Forms$DatePicker: DatePicker,
  Forms$CheckBox: CheckBox,
  Forms$DropDown: Selector,
  Forms$ReferenceSelector: Selector,
  Forms$RadioButtonGroup: Selector,
  Forms$DataView: DataView,
  Forms$ListView: ListView,
  Forms$TemplateGrid: ListView,
  Forms$GroupBox: GroupBox,
  Forms$TabControl: TabControl,
  Forms$TabPage: Contents,
  Forms$NavigationTree: Menu,
  Forms$MenuBar: Menu,
  Forms$SimpleMenuBar: Menu,
  Forms$SidebarToggleButton: SidebarToggle,
  Forms$Header: Header,
  Forms$Title: Title,
  Forms$StaticImageViewer: Image,
  Forms$Table: Table,
  Forms$NavigationList: ListView,
  Forms$NavigationListItem: NavigationItem,
  Forms$DataGrid: DataGrid,
  Forms$GridControlBar: Contents,
  Forms$GridSearchButton: GridButton,
  Forms$GridNewButton: GridButton,
  Forms$GridEditButton: GridButton,
  Forms$GridDeleteButton: GridButton,
  Forms$GridActionButton: GridButton,
  Forms$DataGridRemoveButton: GridButton,
  Forms$DataGridSelectButton: GridButton,
  Forms$DataGridAddButton: GridButton,
  CustomWidgets$CustomWidget: CustomWidget,
  CustomWidgets$WidgetValue: Contents,
};

/** Draws an element the way its stored type is drawn. */
export function render(source: Source): ReactElement {
  const Drawn = drawn[source.type] ?? Box;
  return <Drawn {...source} />;
}
