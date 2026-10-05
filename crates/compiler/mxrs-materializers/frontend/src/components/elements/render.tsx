// How each element of the page vocabulary is drawn. An element is the
// document the model stores; what the browser shows for it is decided here,
// by its stored type. A type nobody draws yet is a box holding its children.
import {
  Children,
  Fragment,
  isValidElement,
  useContext,
  useState,
  type ReactElement,
  type ReactNode,
} from 'react';

import { invokeAction } from '@/api/actions';
import { Navigation } from '@/components/layout/Navigation';

import { Placeholders, Shell } from './context';
import { child, className, content, lastName, plain, sourceOf, text, type Source } from './view';

type Draw = (source: Source) => ReactElement | null;

/** The element a field states, drawn. */
const held = (source: Source, field: string): ReactNode => {
  const stated = source.props[field];
  return isValidElement(stated) ? stated : null;
};

/** What happens when the user acts on a widget: the action its field holds. */
function useAction(source: Source, field: string): (() => void) | undefined {
  const { open } = useContext(Shell);
  const action = child(source, field);
  if (!action) return undefined;
  const settings = (name: string, target: string) => {
    const held = child(action, name);
    return held ? plain(held, target, '') : '';
  };
  const run = (kind: string, handler: string) => () => {
    invokeAction({ kind, handler }, null).catch(console.error);
  };
  switch (action.type) {
    case 'Forms$FormAction': {
      const page = settings('formSettings', 'form');
      return page ? () => open(page) : undefined;
    }
    case 'Forms$MicroflowAction': {
      const microflow = settings('microflowSettings', 'microflow');
      return microflow ? run('microflow', microflow) : undefined;
    }
    case 'Forms$CallNanoflowClientAction': {
      const nanoflow = plain(action, 'nanoflow', '');
      return nanoflow ? run('nanoflow', nanoflow) : undefined;
    }
    case 'Forms$ClosePageClientAction':
    case 'Forms$CancelChangesClientAction':
      return () => history.back();
    default:
      return undefined;
  }
}

const Box: Draw = (source) => (
  <div data-element={source.type} className={className(source)}>
    {content(source)}
  </div>
);

const Contents: Draw = (source) => <>{content(source)}</>;

const Page: Draw = (source) => <>{held(source, 'formCall')}</>;

const Layout: Draw = (source) => (
  <div className={className(source, 'mx-layout')}>{held(source, 'content')}</div>
);

/** A page inside its layout: each argument fills the placeholder it names. */
const LayoutCall: Draw = (source) => {
  // What an argument holds is the caller's: a placeholder in it is one of
  // the layout the caller is itself drawn in.
  const outer = useContext(Placeholders);
  const filled = new Map<string, ReactNode>();
  for (const argument of Children.toArray(content(source))) {
    const stated = sourceOf(argument);
    if (!stated) continue;
    filled.set(
      lastName(plain(stated, 'parameter', '')),
      <Placeholders.Provider value={outer}>{content(stated)}</Placeholders.Provider>,
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
  return <Placeholders.Provider value={filled}>{layout.document}</Placeholders.Provider>;
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

const ScrollContainer: Draw = (source) => (
  <div className={className(source, 'mx-scrollcontainer')}>
    {(['top', 'left', 'centerRegion', 'right', 'bottom'] as const).map((region) => {
      const stated = held(source, region);
      return stated ? (
        <div key={region} className={`mx-scrollcontainer-${region.replace('Region', '')}`}>
          {stated}
        </div>
      ) : null;
    })}
  </div>
);

const Region: Draw = (source) => <div className={className(source)}>{content(source)}</div>;

const DivContainer: Draw = (source) => {
  const act = useAction(source, 'onClickAction');
  return (
    <div className={className(source)} onClick={act} role={act ? 'button' : undefined}>
      {content(source)}
    </div>
  );
};

const LayoutGrid: Draw = (source) => (
  <div className={className(source, 'mx-layoutgrid')}>{content(source)}</div>
);

const LayoutGridRow: Draw = (source) => (
  <div className={className(source, 'row')}>{content(source)}</div>
);

const LayoutGridColumn: Draw = (source) => {
  const weight = plain(source, 'weight', -1);
  const width = weight > 0 ? { flex: `0 0 ${(weight / 12) * 100}%` } : { flex: '1 1 0' };
  return (
    <div className={className(source, 'col')} style={width}>
      {content(source)}
    </div>
  );
};

const DynamicText: Draw = (source) => {
  const mode = plain(source, 'renderMode', 'Text');
  const Tag = (/^H[1-6]$/.test(mode) ? mode.toLowerCase() : mode === 'Paragraph' ? 'p' : 'span') as
    | 'h1'
    | 'p'
    | 'span';
  return <Tag className={className(source, 'mx-text')}>{text(source, 'content')}</Tag>;
};

const StaticText: Draw = (source) => (
  <span className={className(source, 'mx-text')}>{text(source, 'caption')}</span>
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
      className={className(source, 'btn', `btn-${style}`)}
      title={text(source, 'tooltip') || undefined}
      onClick={act}
    >
      {text(source, 'captionTemplate')}
    </button>
  );
};

/** A widget bound to an attribute: its label, and a control of its kind. */
const input =
  (control: (name: string, placeholder: string) => ReactElement): Draw =>
  (source) => {
    const attribute = child(source, 'attributeRef');
    const name = lastName(attribute ? plain(attribute, 'attribute', '') : '');
    const label = text(source, 'labelTemplate');
    return (
      <div className={className(source, 'form-group')}>
        {label ? <label className="control-label">{label}</label> : null}
        {control(name, text(source, 'placeholderTemplate'))}
      </div>
    );
  };

const TextBox = input((name, placeholder) => (
  <input className="form-control" name={name} placeholder={placeholder} />
));
const TextArea = input((name, placeholder) => (
  <textarea className="form-control" name={name} placeholder={placeholder} />
));
const DatePicker = input((name) => <input className="form-control" type="date" name={name} />);
const CheckBox = input((name) => <input type="checkbox" name={name} />);
const Selector = input((name) => <select className="form-control" name={name} />);

const DataView: Draw = (source) => (
  <section className={className(source, 'mx-dataview')}>
    <div className="mx-dataview-content">{content(source)}</div>
    {Array.isArray(source.props.footerWidgets) && source.props.footerWidgets.length ? (
      <footer className="mx-dataview-controls">{each(source.props.footerWidgets)}</footer>
    ) : null}
  </section>
);

const ListView: Draw = (source) => (
  <section className={className(source, 'mx-listview')}>{content(source)}</section>
);

const GroupBox: Draw = (source) => (
  <fieldset className={className(source, 'mx-groupbox')}>
    <legend>{text(source, 'caption')}</legend>
    {content(source)}
  </fieldset>
);

const TabControl: Draw = (source) => {
  const [active, setActive] = useState(0);
  const pages = Children.toArray(content(source)).map(sourceOf);
  const shown = pages[active];
  return (
    <div className={className(source, 'mx-tabcontainer')}>
      <div role="tablist" className="mx-tabcontainer-tabs">
        {pages.map((page, index) => (
          <button
            key={index}
            type="button"
            role="tab"
            aria-selected={index === active}
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

const Menu: Draw = (source) => {
  const { items, open } = useContext(Shell);
  return (
    <div className={className(source, 'mx-navigation')}>
      <Navigation items={items} onOpen={open} />
    </div>
  );
};

const SidebarToggle: Draw = (source) => (
  <button type="button" className={className(source, 'btn')} title={text(source, 'tooltip')}>
    {text(source, 'captionTemplate') || '☰'}
  </button>
);

/** A list of elements a field states, each drawn. */
const each = (value: unknown): ReactNode =>
  Array.isArray(value)
    ? value.map((item, index) => <Fragment key={index}>{item as ReactNode}</Fragment>)
    : null;

const Header: Draw = (source) => (
  <header className={className(source, 'mx-header')}>
    <div>{each(source.props.leftWidgets)}</div>
    <div>{each(source.props.rightWidgets)}</div>
  </header>
);

const Title: Draw = (source) => <h1 className={className(source, 'mx-title')} />;

const Image: Draw = (source) => (
  <span
    role="img"
    className={className(source, 'mx-image')}
    aria-label={lastName(plain(source, 'image', ''))}
  />
);

/** A table: every cell where its row and column say, as wide and tall as it spans. */
const Table: Draw = (source) => (
  <div className={className(source, 'mx-table')}>
    {(Array.isArray(source.props.cells) ? source.props.cells : []).map((cell, index) => {
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
    <div className={className(source, 'mx-navigationlist-item')} onClick={act} role="button">
      {content(source)}
    </div>
  );
};

/** A grid of the model's own kind: its caption, a heading per column, and its buttons. */
const DataGrid: Draw = (source) => (
  <section className={className(source, 'mx-datagrid')}>
    {held(source, 'controlBar')}
    <table>
      <thead>
        <tr>
          {Children.toArray(content(source)).map((column, index) => {
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

/** A pluggable widget: its name, and the widgets its properties hold. */
const CustomWidget: Draw = (source) => {
  const definition = sourceOf(source.defaults.definition);
  const widget = definition ? plain(definition, 'widgetName', '') : '';
  return (
    <section className={className(source, 'mx-widget')} data-widget={widget}>
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
  CustomWidgets$CustomWidget: CustomWidget,
  CustomWidgets$WidgetValue: Contents,
};

/** Draws an element the way its stored type is drawn. */
export function render(source: Source): ReactElement {
  const Drawn = drawn[source.type] ?? Box;
  return <Drawn {...source} />;
}
