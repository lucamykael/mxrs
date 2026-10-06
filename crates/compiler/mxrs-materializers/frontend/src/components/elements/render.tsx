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
import {
  data,
  invoke,
  isObject,
  retrieve,
  ValidationError,
  type DataObject,
  type Query,
} from '@/api/data';
import type { Manifest, RuntimeValue } from '@/types/model';

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
  attributeOf,
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

/** What an attribute's enumeration offers a selector: each value's key, and its caption in the user's language. */
function optionsOf(
  model: Manifest | null,
  entity: string | undefined,
  attribute: string,
): { key: string; caption: string }[] {
  if (!model || !entity || !attribute) return [];
  const declared = model.modules
    .flatMap((module) => module.entities ?? [])
    .find((candidate) => candidate.name === entity)
    ?.attributes.find((candidate) => candidate.name === attribute);
  if (!declared?.enumeration) return [];
  const enumeration = model.modules
    .flatMap((module) => module.enumerations ?? [])
    .find((candidate) => candidate.name === declared.enumeration);
  return (enumeration?.values ?? []).map((value) => ({
    key: value.key,
    caption: textOf(value.caption) || value.key,
  }));
}

// --- conditional visibility: the expression a widget is shown by ---

type Token = { kind: 'word' | 'string' | 'number' | 'op'; text: string };

/** The tokens of a Mendix expression: words and variables, strings, numbers, operators. */
function tokens(expression: string): Token[] {
  const found: Token[] = [];
  const pattern =
    /\s*(?:('(?:[^']|'')*')|(\$[\w/]+|[A-Za-z_][\w]*)|(-?\d+(?:\.\d+)?)|(<=|>=|!=|[=<>()]))/gy;
  let match: RegExpExecArray | null;
  pattern.lastIndex = 0;
  while ((match = pattern.exec(expression)) !== null) {
    if (match[0].trim() === '') break;
    if (match[1] !== undefined)
      found.push({ kind: 'string', text: match[1].slice(1, -1).replace(/''/g, "'") });
    else if (match[2] !== undefined) found.push({ kind: 'word', text: match[2] });
    else if (match[3] !== undefined) found.push({ kind: 'number', text: match[3] });
    else found.push({ kind: 'op', text: match[4] });
    if (pattern.lastIndex >= expression.length) break;
  }
  return found;
}

/**
 * Evaluates the expression a widget is shown by against the object the
 * widget is in and the value its own attribute holds: `$object/Attribute`,
 * `$value`, `empty`, `true`/`false`, numbers, strings, `=`/`!=`/`<`/`>`,
 * `and`/`or`/`not`, parentheses. What it cannot evaluate is `undefined`,
 * and the widget is shown.
 */
function evaluate(expression: string, object: DataObject | null, own: unknown): unknown {
  const list = tokens(expression);
  let at = 0;
  const peek = () => list[at];
  const take = () => list[at++];
  const value = (word: string): unknown => {
    if (word === 'true') return true;
    if (word === 'false') return false;
    if (word === 'empty') return null;
    if (word === '$value') return own ?? null;
    if (word.startsWith('$')) {
      const [, member] = word.split('/');
      if (!member) return object ?? null;
      return object ? (object.members[member] ?? null) : null;
    }
    return undefined;
  };
  const primary = (): unknown => {
    const token = take();
    if (!token) return undefined;
    if (token.kind === 'op' && token.text === '(') {
      const inner = or();
      if (peek()?.text === ')') take();
      return inner;
    }
    if (token.kind === 'string') return token.text;
    if (token.kind === 'number') return Number(token.text);
    if (token.kind === 'word' && token.text === 'not') {
      const operand = primary();
      return operand === undefined ? undefined : !truthy(operand);
    }
    return token.kind === 'word' ? value(token.text) : undefined;
  };
  const same = (left: unknown, right: unknown) =>
    left === right ||
    ((left === null || left === undefined) && (right === null || right === undefined)) ||
    (left !== null && right !== null && String(left) === String(right));
  const comparison = (): unknown => {
    const left = primary();
    const operator = peek();
    if (!operator || operator.kind !== 'op' || operator.text === ')') return left;
    take();
    const right = primary();
    if (left === undefined || right === undefined) return undefined;
    switch (operator.text) {
      case '=':
        return same(left, right);
      case '!=':
        return !same(left, right);
      case '<':
        return Number(left) < Number(right);
      case '>':
        return Number(left) > Number(right);
      case '<=':
        return Number(left) <= Number(right);
      case '>=':
        return Number(left) >= Number(right);
      default:
        return undefined;
    }
  };
  const and = (): unknown => {
    let left = comparison();
    while (peek()?.kind === 'word' && peek().text === 'and') {
      take();
      const right = comparison();
      left = left === undefined || right === undefined ? undefined : truthy(left) && truthy(right);
    }
    return left;
  };
  const or = (): unknown => {
    let left = and();
    while (peek()?.kind === 'word' && peek().text === 'or') {
      take();
      const right = and();
      left = left === undefined || right === undefined ? undefined : truthy(left) || truthy(right);
    }
    return left;
  };
  return or();
}

/** What a Mendix condition holds for true: a true boolean, or a value that is not empty. */
const truthy = (value: unknown): boolean =>
  value !== null && value !== undefined && value !== false && value !== '';

/** Whether the widget is shown: its visibility settings, evaluated; shown when there are none, or they cannot be evaluated. */
function useVisible(source: Source): boolean {
  const row = useContext(Row);
  const draft = useContext(Draft);
  const settings = child(source, 'conditionalVisibilitySettings');
  if (!settings) return true;
  const expression = plain(settings, 'expression', '');
  if (!expression) return true;
  const object = draft?.object ?? row;
  const attribute = child(source, 'attributeRef');
  const own =
    attribute && object ? object.members[lastName(plain(attribute, 'attribute', ''))] : undefined;
  const result = evaluate(expression, object, own);
  return result === undefined ? true : truthy(result);
}

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
/** Leaves the page: back to the one before it, or home when there is none. */
const leave = (open: (page: string) => void) => {
  if (history.length > 1) history.back();
  else open('');
};

function useAction(source: Source, field: string): (() => void) | undefined {
  const { open, changed, fail, notify } = useContext(Shell);
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
  // The object the widget is in: the one a form is filling, or a list's row.
  const here = draft?.object ?? row;
  // What the page gives a flow: the object it is in, for each parameter the
  // settings map to a variable of the page.
  const given = (settings: Source | null): Record<string, RuntimeValue> => {
    const arguments_: Record<string, RuntimeValue> = {};
    for (const mapping of settings ? list(settings, 'parameterMappings').map(sourceOf) : []) {
      if (!mapping || !here) continue;
      const parameter = lastName(plain(mapping, 'parameter', ''));
      const expression = plain(mapping, 'expression', '');
      if (
        parameter &&
        (child(mapping, 'variable') || expression === '$currentObject' || !expression)
      ) {
        arguments_[parameter] = here as unknown as RuntimeValue;
      }
    }
    return arguments_;
  };
  // What a flow asks the page to do once it has run.
  const apply = (effects: Array<Record<string, unknown> & { type: string }>) => {
    for (const effect of effects) {
      switch (effect.type) {
        case 'show_message':
          notify(String(effect.message ?? ''), String(effect.level ?? 'info'));
          break;
        case 'validation_feedback':
          // Feedback about the object the form holds goes under its input;
          // about anything else, it is said as a message.
          if (draft?.object && effect.object_id === draft.object.id && effect.member) {
            draft.rejected([
              { member: String(effect.member), message: String(effect.message ?? '') },
            ]);
          } else {
            notify(String(effect.message ?? ''), 'warning');
          }
          break;
        case 'close_page':
          leave(open);
          break;
        case 'show_home_page':
          open('');
          break;
        case 'show_page':
          if (typeof effect.name === 'string' && effect.name)
            open(effect.name, here ? { [lastName(here.entity)]: here } : {});
          break;
        default:
          break;
      }
    }
    changed();
  };
  const run = (kind: string, handler: string, asking: Source | null) => () => {
    const confirmation = asking ? child(asking, 'confirmationInfo') : null;
    const question = confirmation ? text(confirmation, 'question') : '';
    if (question && !window.confirm(question)) return;
    invokeAction({ kind, handler, arguments: given(asking) }, null)
      .then((answer) => answer && apply(answer.effects))
      .catch(fail);
  };
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
        data<DataObject & { effects?: Array<Record<string, unknown> & { type: string }> }>('save', {
          entity,
          id,
          new: draft.object?.new === true,
          members: sent,
        })
          .then(({ effects, ...saved }) => {
            draft.saved(saved);
            changed();
            if (effects?.length) apply(effects);
            if (plain(action, 'closePage', true)) leave(open);
          })
          .catch((error: unknown) => {
            // What the runtime refused is shown where it belongs, under
            // the inputs; the page stays for the user to put it right.
            if (!(error instanceof ValidationError)) throw error;
            draft.rejected(error.violations);
          }),
      );
    }
    case 'Forms$DeleteClientAction': {
      if (!here || here.new) return undefined;
      const { entity, id } = here;
      const confirm = once(() =>
        data('delete', { entity, id }).then(() => {
          changed();
          if (draft?.object && plain(action, 'closePage', true)) leave(open);
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
      return () => leave(open);
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
  /** What the attribute's enumeration offers, when it holds one. */
  options: { key: string; caption: string }[];
};

/** A widget bound to an attribute: its label, and a control of its kind holding the form's value. */
const input =
  (control: (bound: Bound) => ReactElement): Draw =>
  (source) => {
    const draft = useContext(Draft);
    const { model } = useContext(Shell);
    const attribute = child(source, 'attributeRef');
    const name = lastName(attribute ? plain(attribute, 'attribute', '') : '');
    const label = text(source, 'labelTemplate');
    // What the runtime refused of the member is said under its input, as
    // the Mendix client does, until the user changes it.
    const refused = draft?.violations.get(name);
    // Laid out as the Mendix client lays out a horizontal form: the label
    // in a column of its own and the control, with what was refused of it,
    // in the rest; without a label, the control takes the whole row.
    return (
      <div
        className={className(
          source,
          'form-group',
          label ? '' : 'no-columns',
          refused ? 'has-error' : '',
        )}
      >
        {label ? <label className="control-label col-sm-3">{label}</label> : null}
        <div className={label ? 'col-sm-9' : 'col-sm-12'}>
          {control({
            name,
            placeholder: text(source, 'placeholderTemplate'),
            value: draft?.object?.members[name],
            change: (value) => draft?.set(name, value),
            options: optionsOf(model, draft?.object?.entity, name),
          })}
          {refused ? (
            <div className="alert alert-danger mx-validation-message">{refused}</div>
          ) : null}
        </div>
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
/** A drop-down over an attribute: the enumeration's values, or nothing chosen. */
const Selector = input(({ name, value, change, options }) => (
  <select
    className="form-control"
    name={name}
    value={written(value)}
    onChange={(event) => change(event.target.value || null)}
  >
    <option value="" />
    {options.map((option) => (
      <option key={option.key} value={option.key}>
        {option.caption}
      </option>
    ))}
  </select>
));

/** A radio button per value of the attribute's enumeration. */
const Radios = input(({ name, value, change, options }) => (
  <div className="mx-radiogroup">
    {options.map((option) => (
      <label key={option.key} className="radio-inline">
        <input
          type="radio"
          name={name}
          value={option.key}
          checked={written(value) === option.key}
          onChange={() => change(option.key)}
        />
        {option.caption}
      </label>
    ))}
  </div>
));

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
  const [violations, setViolations] = useState<ReadonlyMap<string, string>>(new Map());
  // A view over a microflow shows the object the flow answers with; a flow
  // the runtime cannot run is said in the view's place, not the page's.
  const settings =
    origin?.type === 'Forms$MicroflowSource' ? child(origin, 'microflowSettings') : null;
  const microflow = settings ? plain(settings, 'microflow', '') : '';
  const [unanswered, setUnanswered] = useState<string>();
  useEffect(() => {
    if (!microflow) return;
    let current = true;
    invoke<unknown>('microflow', microflow, {})
      .then((answer) => {
        const held = (answer as { result?: unknown } | null)?.result ?? answer;
        if (current && isObject(held)) setObject(held);
      })
      .catch((error: unknown) => {
        if (current) setUnanswered(error instanceof Error ? error.message : String(error));
      });
    return () => {
      current = false;
    };
  }, [microflow]);
  const set = (member: string, value: unknown) => {
    setObject((now) => (now ? { ...now, members: { ...now.members, [member]: value } } : now));
    setChanged((now) => new Set(now).add(member));
    // Changed, a member is no longer what the runtime refused.
    setViolations((now) => {
      if (!now.has(member)) return now;
      const next = new Map(now);
      next.delete(member);
      return next;
    });
  };
  const saved = (kept: DataObject) => {
    setObject(kept);
    setChanged(new Set());
    setViolations(new Map());
  };
  const rejected = (refused: { member: string; message: string }[]) =>
    setViolations((now) => {
      const next = new Map(now);
      for (const { member, message } of refused) next.set(member, message);
      return next;
    });
  if (unanswered && !object) {
    return (
      <section className={className(source, 'mx-dataview')} title={unanswered}>
        <div className="mx-dataview-empty">{text(source, 'noEntityMessage')}</div>
      </section>
    );
  }
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
    <Draft.Provider value={{ object, changed, set, saved, violations, rejected }}>
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
  const objects = useObjects(entity, queryOf(source.props.dataSource));
  // So many at a time, as the list says, with the way to the rest.
  const size = Math.max(1, plain(source, 'pageSize', 10));
  const [shownCount, setShownCount] = useState(size);
  // A list about no entity this page can read shows its widgets once.
  if (!entity) {
    return <section className={className(source, 'mx-listview')}>{content(source)}</section>;
  }
  const all = objects ?? [];
  return (
    <section className={className(source, 'mx-listview')} aria-busy={objects === undefined}>
      <ul>
        {all.slice(0, shownCount).map((object) => (
          <li key={object.id} className="mx-listview-item">
            {/* A row is about its own object, whatever form the list is in. */}
            <Row.Provider value={object}>
              <Draft.Provider value={null}>{content(source)}</Draft.Provider>
            </Row.Provider>
          </li>
        ))}
      </ul>
      {all.length > shownCount ? (
        <button
          type="button"
          className="btn mx-button btn-default mx-listview-loadMore"
          onClick={() => setShownCount((now) => now + size)}
        >
          Load more
        </button>
      ) : null}
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

/** How a data source asks for its objects: its XPath constraint and its sort bar. */
function queryOf(value: unknown): Query {
  const stated = sourceOf(value);
  if (!stated) return {};
  const constraint = plain(stated, 'xPathConstraint', '');
  const bar = child(stated, 'sortBar');
  const sort = (bar ? list(bar, 'sortItems') : [])
    .map(sourceOf)
    .filter((item): item is Source => item !== null)
    .map((item) => ({
      attribute: lastName(attributeOf(item)),
      descending: plain(item, 'sortOrder', 'Ascending') === 'Descending',
    }))
    .filter((item) => item.attribute);
  return { ...(constraint ? { constraint } : {}), ...(sort.length ? { sort } : {}) };
}

/**
 * The objects of an entity, as its source asks for them, read again
 * whenever the data changes; `undefined` while they are on their way, and
 * none for no entity.
 */
function useObjects(entity: string, query: Query = {}): DataObject[] | undefined {
  const { changes, fail } = useContext(Shell);
  const [objects, setObjects] = useState<DataObject[]>();
  const asked = JSON.stringify(query);
  useEffect(() => {
    if (!entity) return;
    let current = true;
    retrieve(entity, JSON.parse(asked) as Query)
      .then((answer) => current && setObjects(answer.objects))
      .catch((error) => current && (setObjects([]), fail(error)));
    return () => {
      current = false;
    };
    // `fail` is the application's own and does not change what is listed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entity, asked, changes]);
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

/** How a list's items are selected, as its widget says: not at all, one, or several. */
const selectionOf = (source: Source): 'None' | 'Single' | 'Multi' => {
  const stated = sourceOf(property(source, 'itemSelection'));
  const mode = stated ? plain(stated, 'selection', 'None') : 'None';
  return mode === 'Single' || mode === 'Multi' ? mode : 'None';
};

/** Which of a list's items are selected, and how a click changes that. */
function useSelection(source: Source) {
  const mode = selectionOf(source);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const toggle = (id: string) =>
    setSelected((now) => {
      const next = new Set(mode === 'Multi' ? now : []);
      if (now.has(id) && (mode === 'Multi' || now.size === 1)) next.delete(id);
      else next.add(id);
      return next;
    });
  return { mode, selected, toggle };
}

/**
 * One item of a list, drawn as the widgets inside about its object: a
 * click selects it, or does what the list's `onClick` says.
 */
const ListItem = ({
  source,
  object,
  className: classes,
  selected,
  select,
  children,
}: {
  source: Source;
  object: DataObject;
  className: string;
  selected: boolean;
  select?: () => void;
  children: ReactNode;
}) => (
  <OfRow object={object}>
    <ListItemBody source={source} className={classes} selected={selected} select={select}>
      {children}
    </ListItemBody>
  </OfRow>
);

const ListItemBody = ({
  source,
  className: classes,
  selected,
  select,
  children,
}: {
  source: Source;
  className: string;
  selected: boolean;
  select?: () => void;
  children: ReactNode;
}) => {
  const act = useAction({ ...source, props: { onClick: property(source, 'onClick') } }, 'onClick');
  const double = property(source, 'onClickTrigger') === 'double';
  const clickable = Boolean(act || select);
  const handle = () => {
    select?.();
    act?.();
  };
  return (
    <div
      className={[classes, selected ? `${classes}-selected` : '', clickable ? 'clickable' : '']
        .filter(Boolean)
        .join(' ')}
      role={clickable ? 'button' : undefined}
      onClick={double ? undefined : clickable ? handle : undefined}
      onDoubleClick={double && clickable ? handle : undefined}
    >
      {children}
    </div>
  );
};

/** The rows in the order a column's sort says: by the member's number, or its text. */
function sorted(
  rows: DataObject[],
  columns: Stated[],
  sort: { index: number; descending: boolean } | null,
): DataObject[] {
  if (!sort) return rows;
  const column = columns[sort.index];
  if (!column) return rows;
  const direction = sort.descending ? -1 : 1;
  return [...rows].sort((left, right) => {
    const a = memberOf(left, column.attribute);
    const b = memberOf(right, column.attribute);
    if (typeof a === 'number' && typeof b === 'number') return (a - b) * direction;
    return shown(a).localeCompare(shown(b)) * direction;
  });
}

/** The widgets a pluggable widget's property holds, drawn. */
const inside = (value: unknown): ReactNode => (isValidElement(value) ? value : each(value));

/**
 * Mendix's Data grid 2: the objects of its entity, a row each, in the
 * columns the page states — an attribute's value, a text, or widgets of
 * the page's own — with each column's filter and the grid's pages.
 */
const Datagrid: Draw = (source) => {
  const entity = sourceEntity(property(source, 'datasource'));
  const objects = useObjects(entity, queryOf(property(source, 'datasource')));
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
  const [sort, setSort] = useState<{ index: number; descending: boolean } | null>(null);
  const sortable = (column: Stated) =>
    property(source, 'columnsSortable') !== false &&
    column.sortable !== false &&
    typeof column.attribute === 'string' &&
    column.attribute !== '';
  const sortBy = (index: number) =>
    setSort((now) =>
      now?.index === index
        ? now.descending
          ? null
          : { index, descending: true }
        : { index, descending: false },
    );
  const { mode, selected, toggle } = useSelection(source);
  const byClick = mode !== 'None' && property(source, 'itemSelectionMethod') !== 'checkbox';
  const ordered = sorted(rows, columns, sort);
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
    <div
      className={className(
        source,
        'widget-datagrid',
        byClick ? 'widget-datagrid-selection-method-click' : '',
      )}
    >
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
                <div
                  className={sortable(column) ? 'th clickable' : 'th'}
                  role="columnheader"
                  key={index}
                  aria-sort={
                    sort?.index === index
                      ? sort.descending
                        ? 'descending'
                        : 'ascending'
                      : undefined
                  }
                  onClick={sortable(column) ? () => sortBy(index) : undefined}
                >
                  <div className="column-container">
                    <div className="column-header">
                      {textOf(column.header)}
                      {sort?.index === index ? (sort.descending ? ' ▾' : ' ▴') : null}
                    </div>
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
            {ordered.slice(paging.first, paging.last).map((object) => (
              <ListItem
                source={source}
                object={object}
                className="tr"
                selected={selected.has(object.id)}
                select={byClick ? () => toggle(object.id) : undefined}
                key={object.id}
              >
                {columns.map((column, index) => (
                  <div className={column.wrapText ? 'td wrap-text' : 'td'} role="cell" key={index}>
                    {cell(column, object)}
                  </div>
                ))}
              </ListItem>
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
  const objects = useObjects(entity, queryOf(property(source, 'datasource')));
  const [filter, setFilter] = useState<Filter>();
  const items = (objects ?? []).filter(
    (object) =>
      !filter?.value || Object.values(object.members).some((value) => matches(value, filter)),
  );
  const paging = usePaging(items.length, source);
  const { mode, selected, toggle } = useSelection(source);
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
          <ListItem
            source={source}
            object={object}
            className="widget-gallery-item"
            selected={selected.has(object.id)}
            select={mode !== 'None' ? () => toggle(object.id) : undefined}
            key={object.id}
          >
            {inside(property(source, 'content'))}
          </ListItem>
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
  const { model } = useContext(Shell);
  const options = optionsOf(model, draft?.object?.entity, name);
  // In the group a form's control is in, which is what a theme styles.
  return (
    <div className={className(source, 'form-group')}>
      <div className="widget-combobox">
        <div className="form-control widget-combobox-input-container">
          {options.length ? (
            <select
              className="widget-combobox-input"
              name={name || undefined}
              value={written(value)}
              onChange={(event) => draft?.set(name, event.target.value || null)}
            >
              <option value="">{widgetText(source, 'emptyOptionText')}</option>
              {options.map((option) => (
                <option key={option.key} value={option.key}>
                  {option.caption}
                </option>
              ))}
            </select>
          ) : (
            <input
              className="widget-combobox-input"
              name={name || undefined}
              placeholder={widgetText(source, 'emptyOptionText')}
              value={written(value)}
              readOnly={!name || !draft}
              onChange={(event) => name && draft?.set(name, event.target.value)}
            />
          )}
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

/** The day, month or year an object's date falls in, as a timeline groups events. */
function period(value: unknown, by: unknown): string {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}/.test(value)) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  switch (by) {
    case 'year':
      return date.toLocaleDateString(undefined, { year: 'numeric', timeZone: 'UTC' });
    case 'month':
      return date.toLocaleDateString(undefined, {
        month: 'long',
        year: 'numeric',
        timeZone: 'UTC',
      });
    default:
      return date.toLocaleDateString(undefined, { timeZone: 'UTC' });
  }
}

/**
 * Mendix's Timeline: the objects of its source, an event each — its title,
 * description and time said by the page — grouped by the date the page
 * names, as its own stylesheet lays them out.
 */
const Timeline: Draw = (source) => {
  const entity = sourceEntity(property(source, 'data'));
  const objects = useObjects(entity, queryOf(property(source, 'data')));
  const grouped = property(source, 'groupEvents') !== false;
  // Drawn as the page says, widget by widget, or as the widget's own texts.
  const custom = property(source, 'customVisualization') === true;
  const groups = new Map<string, DataObject[]>();
  for (const object of objects ?? []) {
    const key = grouped
      ? period(memberOf(object, property(source, 'groupAttribute')), property(source, 'groupByKey'))
      : '';
    groups.set(key, [...(groups.get(key) ?? []), object]);
  }
  return (
    <div
      className={className(source, 'widget-timeline-wrapper')}
      aria-busy={entity !== '' && objects === undefined}
    >
      {[...groups].map(([header, events]) => (
        <div key={header || 'all'}>
          {header ? (
            <div className="widget-timeline-date-header">
              {custom && property(source, 'customGroupHeader') ? (
                <OfRow object={events[0]}>{inside(property(source, 'customGroupHeader'))}</OfRow>
              ) : (
                header
              )}
            </div>
          ) : null}
          <div className="widget-timeline-events-wrapper">
            <ul>
              {events.map((object) => (
                <ListItem
                  source={source}
                  object={object}
                  className="widget-timeline-event"
                  selected={false}
                  key={object.id}
                >
                  <div className="widget-timeline-flex-container">
                    <div className="widget-timeline-icon-wrapper">
                      {custom && property(source, 'customIcon') ? (
                        inside(property(source, 'customIcon'))
                      ) : (
                        <div className="widget-timeline-icon-circle" />
                      )}
                    </div>
                    <div className="widget-timeline-content-wrapper">
                      <div className="widget-timeline-info-wrapper">
                        {custom ? (
                          <>
                            {inside(property(source, 'customTitle'))}
                            {inside(property(source, 'customDescription'))}
                          </>
                        ) : (
                          <>
                            <p className="widget-timeline-title">
                              {widgetText(source, 'title', object)}
                            </p>
                            <p className="widget-timeline-description">
                              {widgetText(source, 'description', object)}
                            </p>
                          </>
                        )}
                      </div>
                      <div className="widget-timeline-date-time-wrapper">
                        {custom ? (
                          inside(property(source, 'customEventDateTime'))
                        ) : (
                          <span>{widgetText(source, 'timeIndication', object)}</span>
                        )}
                      </div>
                    </div>
                  </div>
                </ListItem>
              ))}
            </ul>
          </div>
        </div>
      ))}
    </div>
  );
};

/** A point of a chart's series: what the page's X attribute says, and the number its Y attribute holds. */
type Point = { x: string; y: number };

/** Reads one series' objects and hands its points up to the chart. */
const SeriesPoints = ({
  entity,
  x,
  y,
  onPoints,
}: {
  entity: string;
  x: unknown;
  y: unknown;
  onPoints: (points: Point[]) => void;
}) => {
  const objects = useObjects(entity);
  useEffect(() => {
    if (!objects) return;
    onPoints(
      objects.map((object) => ({
        x: shown(memberOf(object, x)),
        y: Number(memberOf(object, y)) || 0,
      })),
    );
    // `onPoints` is the chart's own setter and does not change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [objects]);
  return null;
};

/** The size a chart widget asks for, in the units the page states. */
function chartSize(source: Source): { width: string; height: number } {
  const width = Number(property(source, 'width') ?? 100) || 100;
  const height = Number(property(source, 'height') ?? 75) || 75;
  return {
    width: property(source, 'widthUnit') === 'pixels' ? `${width}px` : `${width}%`,
    height: property(source, 'heightUnit') === 'pixels' ? height : 300,
  };
}

/** A series of a bar, column or line chart, as the page states it: its source, its two attributes, how its points add up. */
function seriesOf(
  source: Source,
): { name: string; entity: string; x: unknown; y: unknown; aggregation: string }[] {
  // A line chart calls its series lines.
  return statedList(property(source, 'series') ?? property(source, 'lines')).map((series) => {
    const dynamic = series.dataSet === 'dynamic';
    return {
      name: textOf(dynamic ? series.dynamicName : series.staticName),
      entity: sourceEntity(dynamic ? series.dynamicDataSource : series.staticDataSource),
      x: dynamic ? series.dynamicXAttribute : series.staticXAttribute,
      y: dynamic ? series.dynamicYAttribute : series.staticYAttribute,
      aggregation: typeof series.aggregationType === 'string' ? series.aggregationType : 'none',
    };
  });
}

/** The points of a series with one x each, their y's added up the way the series says. */
function aggregated(points: Point[], how: string): Point[] {
  if (how === 'none') return points;
  const groups = new Map<string, number[]>();
  for (const point of points) groups.set(point.x, [...(groups.get(point.x) ?? []), point.y]);
  const of = (values: number[]): number => {
    const sorted = [...values].sort((a, b) => a - b);
    switch (how) {
      case 'count':
        return values.length;
      case 'avg':
        return values.reduce((sum, value) => sum + value, 0) / values.length;
      case 'min':
        return sorted[0];
      case 'max':
        return sorted[sorted.length - 1];
      case 'median':
        return sorted[Math.floor(sorted.length / 2)];
      case 'first':
        return values[0];
      case 'last':
        return values[values.length - 1];
      case 'mode': {
        const counts = new Map<number, number>();
        for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1);
        return [...counts].sort((a, b) => b[1] - a[1])[0][0];
      }
      default:
        return values.reduce((sum, value) => sum + value, 0);
    }
  };
  return [...groups].map(([x, values]) => ({ x, y: of(values) }));
}

const PALETTE = ['#264ae5', '#3cb33d', '#eca51c', '#e33f4e', '#7b61ff', '#0ca5b0', '#8a8f9a'];

/**
 * Mendix's bar, column and line charts: each series' points from its own
 * objects, drawn as the bars or the line they make, with the axes' labels
 * and a legend. A chart with nothing to draw yet is its box.
 */
const chart =
  (kind: 'bar' | 'column' | 'line'): Draw =>
  (source) => {
    const series = seriesOf(source);
    const [points, setPoints] = useState<Record<number, Point[]>>({});
    const { width, height } = chartSize(source);
    const categories = [
      ...new Set(series.flatMap((_, index) => (points[index] ?? []).map((point) => point.x))),
    ];
    const top = Math.max(
      1,
      ...series.flatMap((_, index) => (points[index] ?? []).map((point) => point.y)),
    );
    const padding = { left: 48, bottom: 28, top: 12, right: 12 };
    const plotWidth = 600;
    const plotHeight = height - padding.top - padding.bottom;
    const slot = categories.length
      ? (plotWidth - padding.left - padding.right) / categories.length
      : 0;
    const scale = (value: number) => (plotHeight * value) / top;
    const x = (index: number) => padding.left + slot * index;
    const y = (value: number) => padding.top + plotHeight - scale(value);
    return (
      <div
        className={className(source, 'widget-charts', `widget-${kind}-chart`)}
        style={{ width, height: `${height}px` }}
      >
        {series.map((one, index) => (
          <SeriesPoints
            key={index}
            entity={one.entity}
            x={one.x}
            y={one.y}
            onPoints={(found) =>
              setPoints((now) => ({ ...now, [index]: aggregated(found, one.aggregation) }))
            }
          />
        ))}
        <svg viewBox={`0 0 ${plotWidth} ${height}`} width="100%" height={height} role="img">
          <line
            x1={padding.left}
            y1={padding.top}
            x2={padding.left}
            y2={padding.top + plotHeight}
            stroke="#8a8f9a"
          />
          <line
            x1={padding.left}
            y1={padding.top + plotHeight}
            x2={plotWidth - padding.right}
            y2={padding.top + plotHeight}
            stroke="#8a8f9a"
          />
          <text x={padding.left - 6} y={padding.top + 10} textAnchor="end" fontSize="10">
            {top}
          </text>
          <text x={padding.left - 6} y={padding.top + plotHeight} textAnchor="end" fontSize="10">
            0
          </text>
          {categories.map((category, index) => (
            <text
              key={category}
              x={x(index) + slot / 2}
              y={height - 8}
              textAnchor="middle"
              fontSize="10"
            >
              {category}
            </text>
          ))}
          {series.map((one, which) => {
            const mine = points[which] ?? [];
            const color = PALETTE[which % PALETTE.length];
            if (kind === 'line') {
              const path = categories
                .map((category, index) => {
                  const point = mine.find((candidate) => candidate.x === category);
                  return point ? `${x(index) + slot / 2},${y(point.y)}` : null;
                })
                .filter(Boolean)
                .join(' ');
              return (
                <polyline key={which} points={path} fill="none" stroke={color} strokeWidth="2" />
              );
            }
            const barWidth = slot / (series.length + 1);
            return categories.map((category, index) => {
              const point = mine.find((candidate) => candidate.x === category);
              if (!point) return null;
              const barHeight = scale(point.y);
              return kind === 'column' ? (
                <rect
                  key={`${which}-${category}`}
                  x={x(index) + barWidth * (which + 0.5)}
                  y={y(point.y)}
                  width={barWidth}
                  height={barHeight}
                  fill={color}
                />
              ) : (
                <rect
                  key={`${which}-${category}`}
                  x={padding.left}
                  y={x(index) + barWidth * (which + 0.5) - padding.left + padding.top}
                  width={(plotWidth - padding.left - padding.right) * (point.y / top)}
                  height={barWidth}
                  fill={color}
                />
              );
            });
          })}
        </svg>
        {property(source, 'showLegend') !== false && series.some((one) => one.name) ? (
          <ul className="widget-charts-legend">
            {series.map((one, index) => (
              <li key={index}>
                <span
                  style={{ background: PALETTE[index % PALETTE.length] }}
                  className="widget-charts-legend-swatch"
                />
                {one.name}
              </li>
            ))}
          </ul>
        ) : null}
      </div>
    );
  };

/** Mendix's pie chart: a slice per object of its source, as big as the value attribute says. */
const PieChart: Draw = (source) => {
  const entity = sourceEntity(property(source, 'seriesDataSource'));
  const objects = useObjects(entity);
  const { width, height } = chartSize(source);
  const slices = (objects ?? []).map((object) => ({
    name:
      widgetText(source, 'seriesName', object) ||
      shown(memberOf(object, property(source, 'seriesName'))),
    value: Math.max(0, Number(memberOf(object, property(source, 'seriesValueAttribute'))) || 0),
  }));
  const total = slices.reduce((sum, slice) => sum + slice.value, 0) || 1;
  const hole = Number(property(source, 'holeRadius') ?? 0) || 0;
  let angle = -Math.PI / 2;
  const radius = 90;
  const arcs = slices.map((slice, index) => {
    const span = (slice.value / total) * 2 * Math.PI;
    const start = angle;
    angle += span;
    const point = (at: number, r: number) => `${100 + r * Math.cos(at)},${100 + r * Math.sin(at)}`;
    const large = span > Math.PI ? 1 : 0;
    const inner = (radius * hole) / 100;
    const d = inner
      ? `M ${point(start, radius)} A ${radius} ${radius} 0 ${large} 1 ${point(angle, radius)} L ${point(angle, inner)} A ${inner} ${inner} 0 ${large} 0 ${point(start, inner)} Z`
      : `M 100,100 L ${point(start, radius)} A ${radius} ${radius} 0 ${large} 1 ${point(angle, radius)} Z`;
    return (
      <path key={index} d={d} fill={PALETTE[index % PALETTE.length]}>
        <title>{`${slice.name}: ${slice.value}`}</title>
      </path>
    );
  });
  return (
    <div
      className={className(source, 'widget-charts', 'widget-pie-chart')}
      style={{ width, height: `${height}px` }}
    >
      <svg viewBox="0 0 200 200" width="100%" height={height} role="img">
        {arcs}
      </svg>
      {property(source, 'showLegend') !== false ? (
        <ul className="widget-charts-legend">
          {slices.map((slice, index) => (
            <li key={index}>
              <span
                style={{ background: PALETTE[index % PALETTE.length] }}
                className="widget-charts-legend-swatch"
              />
              {slice.name}
            </li>
          ))}
        </ul>
      ) : null}
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
  'com.mendix.widget.web.timeline.Timeline': Timeline,
  'com.mendix.widget.web.barchart.BarChart': chart('bar'),
  'com.mendix.widget.web.columnchart.ColumnChart': chart('column'),
  'com.mendix.widget.web.linechart.LineChart': chart('line'),
  'com.mendix.widget.web.piechart.PieChart': PieChart,
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
  Forms$RadioButtonGroup: Radios,
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
  return (
    <Shown source={source}>
      <Drawn {...source} />
    </Shown>
  );
}

/** What is drawn only while the condition the page states for it holds. */
const Shown = ({ source, children }: { source: Source; children: ReactElement }) =>
  useVisible(source) ? children : null;
