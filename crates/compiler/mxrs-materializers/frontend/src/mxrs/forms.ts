// What pages, layouts and snippets are written with. A page is the document
// the model stores for it, stated as TSX: each element of
// `src/mxrs/elements.ts` is a kind of stored document, each prop one of its
// fields, and the widgets it holds are its children. A prop left unsaid has
// the value `elements.ts` gives it. mxrs reads these files into the model
// on every build; it does not run them. The browser does: an element draws
// itself the way `src/components/elements` says its type is drawn.
import { type ReactElement, type ReactNode } from "react";

import { render } from "@/components/elements/render";

/** A number the model stores in 64 bits where its field holds 32: `long(5)`. */
export interface Long {
  readonly long: number;
}

/** A number the model stores in 32 bits where its field holds 64: `int(5)`. */
export interface Int {
  readonly int: number;
}

/** The element of the same page whose `name` this is: `named("tabPage1")`. */
export interface Named {
  readonly named: string;
}

/** The identity of a document outside the page. */
export interface Identity {
  readonly identity: string;
}

/**
 * Binary data that holds nothing: `binary()`. Data that holds something — a
 * template's thumbnail — is a file beside the page, imported by its name.
 */
export interface Binary {
  readonly binary: true;
}

/** A property the widget's definition has and this use of it does not store. */
export interface Missing {
  readonly missing: true;
}

/**
 * What a prop states: a text, a number, a boolean, nothing, another element,
 * texts by language code (`{ en_US: "Save" }`), a pluggable widget's object
 * by its property keys, or a list of those.
 */
export type Value =
  | string
  | number
  | boolean
  | null
  | ReactElement
  | Long
  | Int
  | Named
  | Identity
  | Binary
  | { [key: string]: Value | Missing }
  | Value[];

/** A list that holds nothing by default; the number is how the model marks it. */
export interface ListDefault {
  readonly list: number;
}

/** The list written as the element's children. */
export interface ChildrenDefault {
  readonly children: number;
}

/**
 * A component of the page vocabulary. Besides its props it carries, for the
 * types alone, the stored type it is (`T`) and what the field it is mostly
 * stated for holds (`M`): a prop that holds the element may be written as
 * that instead.
 */
export type Component<P, M = never, T extends string = string> = ((props: P) => ReactElement) & {
  readonly __main?: M;
  readonly __type?: T;
  /** What the element is, for whoever draws it. */
  readonly element?: Kind;
};

/** What an element is: its stored type, its fields' defaults and its main field. */
export interface Kind {
  readonly type: string;
  readonly defaults: Readonly<Record<string, unknown>>;
  readonly main?: string;
}

/** What a field holds when a page says nothing about it. */
export type Default =
  | string
  | number
  | boolean
  | null
  | Long
  | Binary
  | ListDefault
  | ChildrenDefault
  | Component<never, unknown, string>;

/** Texts by language code: `{ en_US: "Save" }`. */
export type Texts = { [language: string]: string };

/**
 * What a field holds, by what it holds by default: a text where the default
 * is a text, a number where it is a number, a list where it is a list, binary
 * data — `binary()`, or a file imported beside the page — where it is. A
 * field that holds an element by default holds an element, nothing, or —
 * written alone — the one thing that element is mostly stated for; texts by
 * language where the element is a text. A field that holds nothing by
 * default says nothing of its kind.
 */
export type Holds<V> = V extends ListDefault
  ? Value[]
  : V extends Binary
    ? Binary | string
    : V extends string
      ? string
      : V extends number | Long
        ? number | Long | Int
        : V extends boolean
          ? boolean
          : V extends null
            ? Value
            : V extends Component<never, infer M, infer T>
              ? ReactElement | null | M | (T extends "Texts$Text" ? Texts : never)
              : never;

/** An element's props: its fields, each optional, and its children when it holds any. */
export type Props<D> = {
  [K in keyof D as D[K] extends ChildrenDefault ? never : K]?: Holds<D[K]>;
} & (ChildrenDefault extends D[keyof D] ? { children?: ReactNode } : { children?: never });

export function list(marker: number): ListDefault {
  return { list: marker };
}

export function children(marker: number): ChildrenDefault {
  return { children: marker };
}

export function binary(): Binary {
  return { binary: true };
}

export function long(value: number): Long {
  return { long: value };
}

export function int(value: number): Int {
  return { int: value };
}

export function named(name: string): Named {
  return { named: name };
}

export function identity(id: string): Identity {
  return { identity: id };
}

/** The identity that points at nothing. */
export const unset: Identity = { identity: "00000000-0000-0000-0000-000000000000" };

export const missing: Missing = { missing: true };

/**
 * An element: the stored document type it is, what its fields hold by
 * default, and the one field it is mostly stated for — a prop that holds
 * the element and says only that field is written as the field's value.
 */
export function element<
  T extends string,
  D extends Record<string, Default>,
  M extends keyof D & string = never,
>(type: T, defaults: D, main?: M): Component<Props<D>, [M] extends [never] ? never : Holds<D[M]>, T> {
  const kind: Kind = { type, defaults, main };
  const component = (props: Props<D>) => render({ ...kind, props: props as Record<string, unknown> });
  return Object.assign(component, { element: kind });
}

/** A pluggable widget's props: the fields of the widget it is stored as, and its own properties by their keys. */
export type WidgetProps = Record<string, Value> & {
  properties?: Record<string, Value | Missing>;
};

/**
 * A pluggable widget, by its definition and — where a property mostly holds
 * more than its type's own default — what its properties hold when a use of
 * the widget says nothing about them, by their keys (`"columns.header"` for
 * a property of the objects another holds).
 */
export function widget(
  definition: ReactElement,
  defaults?: Record<string, ReactElement>,
): Component<WidgetProps> {
  const kind: Kind = { type: "CustomWidgets$CustomWidget", defaults: { definition, ...defaults } };
  const component = (props: WidgetProps) => render({ ...kind, props });
  return Object.assign(component, { element: kind });
}

/** A page, layout or snippet of a module. */
export interface Form {
  readonly kind: "page" | "layout" | "snippet" | "pageTemplate" | "buildingBlock";
  readonly module: string;
  readonly document: ReactElement;
}

export function page(module: string, document: ReactElement): Form {
  return { kind: "page", module, document };
}

export function layout(module: string, document: ReactElement): Form {
  return { kind: "layout", module, document };
}

export function snippet(module: string, document: ReactElement): Form {
  return { kind: "snippet", module, document };
}

export function pageTemplate(module: string, document: ReactElement): Form {
  return { kind: "pageTemplate", module, document };
}

export function buildingBlock(module: string, document: ReactElement): Form {
  return { kind: "buildingBlock", module, document };
}
