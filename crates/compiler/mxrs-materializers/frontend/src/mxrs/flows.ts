// The vocabulary nanoflows are written in. A nanoflow is an async method of
// a service, `nanoflowService("Module", { ... })`, in
// `src/services/<module>/<subject>Service.ts`: TypeScript's own `if`, `switch`,
// `for...of`, `return`, `break` and `continue` are its control flow, and each
// activity is one of the functions below. mxrs reads these files into the
// model on every build; it does not run them, and neither does the browser
// yet — calling one throws.

/** A Mendix expression, as Studio Pro writes it: `mx("$Order/Total > 5")`. */
export interface MxExpression {
  readonly __mendix: string;
}

/** An object of the entity `E` (`"Module.Entity"`). */
export interface MxObject<E extends string> {
  readonly __object: E;
}

/** A list of objects of the entity `E`; a `for...of` goes over them. */
export interface MxList<E extends string> extends Iterable<MxObject<E>> {
  readonly __list: E;
}

/**
 * What an activity gives back when what it holds is not known here — the
 * result of a call, or what an association leads to. The model knows; the
 * TypeScript does not check it.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type MxResult = any;

/** A value of the enumeration `E` (`"Module.Enumeration"`). */
export interface MxEnum<E extends string> {
  readonly __enumeration: E;
}

export type MxInteger = number;
export type MxLong = number;
export type MxDecimal = number;
export type MxFloat = number;
export type MxDateTime = Date;
export interface MxBinary {
  readonly __binary: true;
}

/** What an activity reads: a text, number or boolean as itself, a variable, or an expression. */
export type MxValue =
  | string
  | number
  | boolean
  | MxExpression
  | MxObject<string>
  | MxList<string>
  | MxEnum<string>
  | Date
  | MxBinary
  | null
  | undefined;

/** What an object's members are set to: an attribute by its name, an association by its qualified name. */
export type Members = Record<string, MxValue>;

/** A list's order: each member with its direction. */
export type Sorting = [member: string, order: "asc" | "desc"][];

/** Texts by language code. */
export type Texts = Record<string, string>;

function notRunHere(): never {
  throw new Error("nanoflows run in Mendix; mxrs reads them, it does not run them in the browser yet");
}

/** The nanoflows of a module about one subject. */
export function nanoflowService<S extends Record<string, (...args: never[]) => Promise<unknown>>>(
  module: string,
  flows: S,
): S {
  void module;
  return flows;
}

/**
 * A Mendix expression: `mx("$Order/Total > 5")`, `mx("empty")`. Its type is
 * whatever the place it stands in needs; Mendix checks the expression.
 */
export function mx<T = MxExpression>(expression: string): T {
  return { __mendix: expression } as unknown as T;
}

/** A variable no identifier holds: `variable("currentObject")`. */
export function variable<T = never>(name: string): T {
  return { __mendix: `$${name}` } as unknown as T;
}

/** Goes over `list` naming each object `name` in the model. */
export function iterate<E extends string>(list: MxList<E>, name: string): Iterable<MxObject<E>> {
  void [list, name];
  return notRunHere();
}

// Objects and lists.

export function createObject<E extends string>(
  entity: E,
  members?: Members,
  options?: { commit?: boolean | "withoutEvents"; refresh?: boolean; name?: string },
): Promise<MxObject<E>> {
  void [entity, members, options];
  return notRunHere();
}

export function changeObject(
  object: MxObject<string>,
  members?: Members,
  options?: {
    commit?: boolean | "withoutEvents";
    refresh?: boolean;
    add?: Members;
    remove?: Members;
  },
): Promise<void> {
  void [object, members, options];
  return notRunHere();
}

export function commitObject(
  object: MxObject<string> | MxList<string>,
  options?: { withEvents?: boolean; refresh?: boolean },
): Promise<void> {
  void [object, options];
  return notRunHere();
}

export function deleteObject(
  object: MxObject<string> | MxList<string>,
  options?: { refresh?: boolean },
): Promise<void> {
  void [object, options];
  return notRunHere();
}

export function rollbackObject(
  object: MxObject<string> | MxList<string>,
  options?: { refresh?: boolean },
): Promise<void> {
  void [object, options];
  return notRunHere();
}

interface RetrieveOptions {
  xpath?: string;
  sort?: Sorting;
  range?: [limit: MxValue, offset: MxValue];
  name?: string;
}

/** The first object of `entity` from the database. */
export function retrieve<E extends string>(
  entity: E,
  options: RetrieveOptions & { first: true },
): Promise<MxObject<E>>;
/** Objects of `entity` from the database. */
export function retrieve<E extends string>(
  entity: E,
  options?: RetrieveOptions & { first?: false },
): Promise<MxList<E>>;
/** What an object is associated with: `{ by: "Module.Association" }`. */
export function retrieve(
  object: MxObject<string>,
  options: { by: string; name?: string },
): Promise<MxResult>;
export function retrieve(from: unknown, options?: unknown): Promise<MxResult> {
  void [from, options];
  return notRunHere();
}

export function createList<E extends string>(
  entity: E,
  options?: { name?: string },
): Promise<MxList<E>> {
  void [entity, options];
  return notRunHere();
}

/** Changes a list: adds, removes or replaces objects, or clears it. */
export function changeList<E extends string>(
  list: MxList<E>,
  operation: { add: MxValue } | { remove: MxValue } | { replace: MxValue } | { clear: true },
): Promise<void>;
/** An object of a list, which `name` names. */
export function changeList<E extends string>(
  list: MxList<E>,
  operation: { head: true } | { find: [member: string, value: MxValue] } | { findBy: MxValue },
  options?: { name?: string },
): Promise<MxObject<E>>;
/** Another list made of a list, which `name` names. */
export function changeList<E extends string>(
  list: MxList<E>,
  operation:
    | { tail: true }
    | { filter: [member: string, value: MxValue] }
    | { filterBy: MxValue }
    | { sort: Sorting }
    | { range: [limit: MxValue, offset: MxValue] }
    | { union: MxList<string> }
    | { intersect: MxList<string> }
    | { subtract: MxList<string> },
  options?: { name?: string },
): Promise<MxList<E>>;
/** Whether a list holds an object, or equals another. */
export function changeList<E extends string>(
  list: MxList<E>,
  operation: { contains: MxObject<string> } | { equals: MxList<string> },
  options?: { name?: string },
): Promise<boolean>;
export function changeList(list: unknown, operation: unknown, options?: unknown): Promise<MxResult> {
  void [list, operation, options];
  return notRunHere();
}

/** Whether all or any object of a list makes it true. */
export function aggregateList<E extends string>(
  list: MxList<E>,
  aggregate: { all: string } | { any: string } | { allOf: MxValue } | { anyOf: MxValue },
  options?: { name?: string },
): Promise<boolean>;
/** A number a list adds up to. */
export function aggregateList<E extends string>(
  list: MxList<E>,
  aggregate:
    | { count: true }
    | { sum: string }
    | { average: string }
    | { minimum: string }
    | { maximum: string }
    | { sumOf: MxValue }
    | { averageOf: MxValue }
    | { minimumOf: MxValue }
    | { maximumOf: MxValue },
  options?: { name?: string },
): Promise<number>;
export function aggregateList(list: unknown, aggregate: unknown, options?: unknown): Promise<MxResult> {
  void [list, aggregate, options];
  return notRunHere();
}

// Variables.

export type VariableType =
  | "String"
  | "Integer"
  | "Long"
  | "Decimal"
  | "Float"
  | "Boolean"
  | "DateTime"
  | "Binary"
  | { object: string }
  | { list: string }
  | { enumeration: string };

export function createVariable(
  type: VariableType,
  value: MxValue,
  options?: { name?: string },
): Promise<MxResult> {
  void [type, value, options];
  return notRunHere();
}

export function changeVariable(variable: unknown, value: MxValue): Promise<void> {
  void [variable, value];
  return notRunHere();
}

// Calls.

export function callMicroflow(
  microflow: string,
  args?: Record<string, MxValue>,
  options?: { name?: string; discardResult?: string; queue?: string },
): Promise<MxResult> {
  void [microflow, args, options];
  return notRunHere();
}

/** A nanoflow called by name; a service's method calls it by itself. */
export function callNanoflow(
  nanoflow: string,
  args?: Record<string, MxValue>,
  options?: { name?: string; discardResult?: string },
): Promise<MxResult> {
  void [nanoflow, args, options];
  return notRunHere();
}

export function callJavaScriptAction(
  action: string,
  args?: Record<string, MxValue | { entity: string } | { nanoflow: string }>,
  options?: { name?: string; discardResult?: string },
): Promise<MxResult> {
  void [action, args, options];
  return notRunHere();
}

// The client.

export function showPage(
  page: string,
  options?: {
    args?: Record<string, MxValue>;
    title?: Texts;
    titleParameters?: MxValue[];
    closePages?: MxValue;
  },
): Promise<void> {
  void [page, options];
  return notRunHere();
}

export function closePage(count?: MxValue): Promise<void> {
  void count;
  return notRunHere();
}

export function showMessage(
  kind: "information" | "warning" | "error",
  text: Texts,
  options?: { parameters?: MxValue[]; blocking?: boolean },
): Promise<void> {
  void [kind, text, options];
  return notRunHere();
}

export function log(
  level: "Trace" | "Debug" | "Info" | "Warning" | "Error" | "Critical",
  node: MxValue,
  message: string,
  options?: { parameters?: MxValue[]; stackTrace?: boolean },
): Promise<void> {
  void [level, node, message, options];
  return notRunHere();
}

// Paths a structured flow cannot say otherwise.

/** Marks where `jump(name)` carries on. */
export function label(name: string): void {
  void name;
}

/** Ends this path by carrying on at `label(name)`. */
export function jump(name: string): never {
  void name;
  return notRunHere();
}

/**
 * Runs `activity` and, when it fails, `handler`: `"custom"` rolls back what
 * the flow changed, `"customWithoutRollback"` keeps it, and `"continue"` has
 * no handler and goes on.
 */
export async function onError<T>(
  handling: "custom" | "customWithoutRollback" | "continue",
  activity: () => Promise<T>,
  handler?: () => Promise<void>,
): Promise<T> {
  void [handling, activity, handler];
  return notRunHere();
}

/** Ends the flow by raising the error being handled to its caller. */
export function raiseError(): never {
  return notRunHere();
}

/** An activity the model keeps but does not run. */
export async function disabled<T>(activity: () => Promise<T>): Promise<T> {
  void activity;
  return notRunHere();
}
