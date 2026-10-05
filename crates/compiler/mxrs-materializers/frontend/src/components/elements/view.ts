// How a drawn element is read: what a field says — stated by the page, or
// the default `mxrs/elements.ts` gives it — whichever way the page wrote it.
import { Children, isValidElement, type ReactElement, type ReactNode } from 'react';

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

/** A number however a page writes it: itself, `long(5)` or `int(5)`. */
function number(value: unknown): unknown {
  if (typeof value !== 'object' || value === null) return value;
  const sized = value as { long?: unknown; int?: unknown };
  return typeof sized.long === 'number' ? sized.long : typeof sized.int === 'number' ? sized.int : value;
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
  const stated = number(source.props[field] ?? source.defaults[field]);
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

/** What an element holds as its children: the one list its page writes inside it. */
export const content = (source: Source): ReactNode => source.props.children as ReactNode;

/**
 * The list a field holds. Which of an element's lists a page writes as its
 * children is the project's own (`elements.ts` marks it), so a list is read
 * from wherever this project's pages state it.
 */
export function list(source: Source, field: string): ReactNode[] {
  const marker = source.defaults[field];
  const inside = typeof marker === 'object' && marker !== null && 'children' in marker;
  if (inside) return Children.toArray(source.props.children as ReactNode);
  const stated = source.props[field];
  return Array.isArray(stated) ? (stated as ReactNode[]) : [];
}

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
export function text(
  source: Source,
  field: string,
  object?: { members: Record<string, unknown> } | null,
): string {
  const stated = source.props[field];
  if (typeof stated === 'string') return stated;
  if (isTexts(stated)) return inLanguage(stated);
  const held = child(source, field);
  if (!held) return '';
  if (held.type === 'Texts$Text') {
    if (isTexts(held.props.texts)) return inLanguage(held.props.texts);
    // Written whole: a translation per language.
    const translations = list(held, 'items').map(sourceOf);
    return inLanguage(
      Object.fromEntries(
        translations.flatMap((translation) =>
          translation
            ? [[plain(translation, 'languageCode', ''), plain(translation, 'text', '')]]
            : [],
        ),
      ),
    );
  }
  if (held.type !== 'Forms$ClientTemplate') return '';
  const parameters = list(held, 'parameters')
    .map(sourceOf)
    .map((parameter) => {
      if (!parameter) return '…';
      const attribute = child(parameter, 'attributeRef');
      const name = lastName(attribute ? plain(attribute, 'attribute', '') : '');
      if (!name) return '…';
      // The value of the object being shown, or the attribute's name where
      // there is none to show.
      return object ? shown(object.members[name]) : `{${name}}`;
    });
  return text(held, 'template').replace(/\{(\d+)\}/g, (_all, index: string) => {
    return parameters[Number(index) - 1] ?? '…';
  });
}

/** A member's value as a page shows it. */
export function shown(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'string' && /^\d{4}-\d{2}-\d{2}T/.test(value)) {
    const instant = new Date(value);
    return Number.isNaN(instant.getTime()) ? value : instant.toLocaleDateString();
  }
  return String(value);
}

/** The entity a data source or an action is about. */
export function entityOf(source: Source, field = 'entityRef'): string {
  const stated = source.props[field];
  if (typeof stated === 'string') return stated;
  const held = child(source, field);
  return held ? plain(held, 'entity', '') : '';
}

/** The class names an element's appearance gives it. */
export function className(source: Source, ...own: string[]): string | undefined {
  const appearance = child(source, 'appearance');
  const stated = appearance ? plain(appearance, 'class', '') : '';
  // Some types store a class of their own besides.
  const names = [...own, stated, plain(source, 'class', '')].join(' ').split(/\s+/).filter(Boolean);
  return [...new Set(names)].join(' ') || undefined;
}

/** The last part of a qualified name: `Sales.Order.Number` is `Number`. */
export const lastName = (qualified: string) => qualified.split('.').pop() ?? qualified;
