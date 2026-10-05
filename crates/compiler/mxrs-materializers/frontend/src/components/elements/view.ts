// How a drawn element is read: what a field says — stated by the page, or
// the default `mxrs/elements.ts` gives it — whichever way the page wrote it.
import { isValidElement, type ReactElement, type ReactNode } from 'react';

import type { Kind } from '@/mxrs/forms';

/** An element being drawn: what it is, and what the page states of it. */
export interface Source extends Kind {
  readonly props: Readonly<Record<string, unknown>>;
}

type Vocabulary = { element?: Kind };

const kindOf = (value: unknown): Kind | undefined =>
  typeof value === 'function' ? (value as Vocabulary).element : undefined;

/** The element a value is, when it is one of the vocabulary. */
export function sourceOf(value: unknown): Source | null {
  if (!isValidElement(value)) return null;
  const kind = kindOf(value.type);
  return kind ? { ...kind, props: (value as ReactElement<Record<string, unknown>>).props } : null;
}

/** A field that holds a text, a number or a boolean. */
export function plain(source: Source, field: string, otherwise: string): string;
export function plain(source: Source, field: string, otherwise: number): number;
export function plain(source: Source, field: string, otherwise: boolean): boolean;
export function plain(
  source: Source,
  field: string,
  otherwise: string | number | boolean,
): string | number | boolean {
  const stated = source.props[field] ?? source.defaults[field];
  return typeof stated === typeof otherwise ? (stated as typeof otherwise) : otherwise;
}

/**
 * The element a field holds. A page may state it whole, leave it to its
 * default, or write only the one thing it is stated for
 * (`appearance="card"`, `captionTemplate={{ en_US: "Save" }}`).
 */
export function child(source: Source, field: string): Source | null {
  const stated = source.props[field];
  const whole = sourceOf(stated);
  if (whole) return whole;
  if (stated === null) return null;
  const kind = kindOf(source.defaults[field]);
  if (!kind) return null;
  if (stated === undefined) return { ...kind, props: {} };
  if (kind.type === 'Texts$Text') return { ...kind, props: { texts: stated } };
  return kind.main ? { ...kind, props: { [kind.main]: stated } } : null;
}

/** The widgets an element holds. */
export const content = (source: Source): ReactNode => source.props.children as ReactNode;

const isTexts = (value: unknown): value is Record<string, string> =>
  typeof value === 'object' &&
  value !== null &&
  !isValidElement(value) &&
  !Array.isArray(value) &&
  Object.values(value).every((text) => typeof text === 'string');

const language = () =>
  typeof document === 'undefined' ? '' : document.documentElement.lang.replace('-', '_');

const inLanguage = (texts: Record<string, string>) =>
  texts[language()] ??
  texts.en_US ??
  Object.values(texts)[0] ??
  '';

/** The text a field says in the user's language: a text, or a template with its parameters named. */
export function text(source: Source, field: string): string {
  const stated = source.props[field];
  if (typeof stated === 'string') return stated;
  if (isTexts(stated)) return inLanguage(stated);
  const held = child(source, field);
  if (!held) return '';
  if (held.type === 'Texts$Text') return isTexts(held.props.texts) ? inLanguage(held.props.texts) : '';
  if (held.type !== 'Forms$ClientTemplate') return '';
  const parameters = (Array.isArray(held.props.parameters) ? held.props.parameters : [])
    .map(sourceOf)
    .map((parameter) => {
      if (!parameter) return '…';
      const attribute = child(parameter, 'attributeRef');
      const name = attribute ? plain(attribute, 'attribute', '') : '';
      return name ? `{${name.split('.').pop()}}` : '…';
    });
  return text(held, 'template').replace(/\{(\d+)\}/g, (_all, index: string) => {
    return parameters[Number(index) - 1] ?? '…';
  });
}

/** The class names an element's appearance gives it. */
export function className(source: Source, ...own: string[]): string | undefined {
  const appearance = child(source, 'appearance');
  const stated = appearance ? plain(appearance, 'class', '') : '';
  return [...own, stated].filter(Boolean).join(' ') || undefined;
}

/** The last part of a qualified name: `Sales.Order.Number` is `Number`. */
export const lastName = (qualified: string) => qualified.split('.').pop() ?? qualified;
