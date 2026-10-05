// What pages, layouts and snippets are written with. A page is the document
// the model stores for it, stated as TSX: each element of
// `src/mxrs/elements.ts` is a kind of stored document, each prop one of its
// fields, and the widgets it holds are its children. A prop left unsaid has
// the value `elements.ts` gives it. mxrs reads these files into the model
// on every build; it does not run them, and the browser does not render
// them yet — an element renders as an empty box that holds its children.
import { createElement, type ReactElement, type ReactNode } from "react";

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
  | { [key: string]: string | ReactElement | Missing }
  | Value[];

/** A list that holds nothing by default; the number is how the model marks it. */
export interface ListDefault {
  readonly list: number;
}

/** The list written as the element's children. */
export interface ChildrenDefault {
  readonly children: number;
}

export type Component<P> = (props: P) => ReactElement;

/** What a field holds when a page says nothing about it. */
export type Default =
  | string
  | number
  | boolean
  | null
  | Long
  | ListDefault
  | ChildrenDefault
  | Component<never>;

/** Texts by language code: `{ en_US: "Save" }`. */
export type Texts = { [language: string]: string };

/**
 * What a field holds, by what it holds by default: a text where the default
 * is a text, a number where it is a number, an element — or texts, or
 * nothing — where it is an element. A field that holds nothing by default
 * says nothing of its kind.
 */
export type Holds<V> = V extends ListDefault
  ? Value[]
  : V extends string
    ? string
    : V extends number | Long
      ? number | Long | Int
      : V extends boolean
        ? boolean
        : V extends null
          ? Value
          : ReactElement | Texts | null;

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

function box(type: string, content: ReactNode): ReactElement {
  return createElement("div", { "data-element": type }, content);
}

/** An element: the stored document type it is, and what its fields hold by default. */
export function element<D extends Record<string, Default>>(
  type: string,
  defaults: D,
): Component<Props<D>> {
  void defaults;
  return (props) => box(type, (props as { children?: ReactNode }).children);
}

/** A pluggable widget's props: the fields of the widget it is stored as, and its own properties by their keys. */
export type WidgetProps = Record<string, Value> & {
  properties?: Record<string, ReactElement | Missing>;
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
  void [definition, defaults];
  return () => box("CustomWidgets$CustomWidget", null);
}

/** A page, layout or snippet of a module. */
export interface Form {
  readonly module: string;
  readonly document: ReactElement;
}

export function page(module: string, document: ReactElement): Form {
  return { module, document };
}

export function layout(module: string, document: ReactElement): Form {
  return { module, document };
}

export function snippet(module: string, document: ReactElement): Form {
  return { module, document };
}
