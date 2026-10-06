import type { ReactElement } from 'react';

import { child, plain, sourceOf, text, type Source } from '@/components/elements/view';

import type { Form } from '@/mxrs/forms';

type Declared = { default?: Form };

let declared: Map<string, Form> | undefined;

/** Every page, layout and snippet the frontend declares, by qualified name. */
function forms(): Map<string, Form> {
  if (declared) return declared;
  const files = import.meta.glob<Declared>(
    ['../pages/*/*.tsx', '../components/layout/*/*.tsx', '../components/snippets/*/*.tsx'],
    { eager: true },
  );
  declared = new Map();
  for (const file of Object.values(files)) {
    const form = file.default;
    const name = (form?.document as ReactElement<{ name?: string }> | undefined)?.props.name;
    if (form && name) declared.set(`${form.module}.${name}`, form);
  }
  return declared;
}

/** The form of that qualified name, when the frontend declares it. */
export const findForm = (qualified: string): Form | undefined => forms().get(qualified);

/**
 * How a page opens: in place of the page it was opened from, or — when the
 * layout it calls is a popup — over it, as a window of the size the page
 * states, with the rest of the application behind an underlay when the
 * popup is modal.
 */
export type Opening =
  { popup: false } | { popup: true; modal: boolean; width: number; height: number; title: string };

export function openingOf(qualified: string): Opening {
  const form = findForm(qualified);
  const page = form?.kind === 'page' ? sourceOf(form.document) : null;
  if (!page) return { popup: false };
  const type = layoutType(child(page, 'formCall'), 0);
  if (type !== 'Popup' && type !== 'ModalPopup') return { popup: false };
  return {
    popup: true,
    modal: type === 'ModalPopup',
    width: plain(page, 'popupWidth', 0),
    height: plain(page, 'popupHeight', 0),
    title: text(page, 'title'),
  };
}

/** The type of the layout a call names: its own, or that of the layout it is based on. */
function layoutType(call: Source | null, depth: number): string {
  if (!call || depth > 8) return '';
  const layout = sourceOf(findForm(plain(call, 'form', ''))?.document);
  const content = layout ? child(layout, 'content') : null;
  if (!content) return '';
  return plain(content, 'layoutType', '') || layoutType(child(content, 'layoutCall'), depth + 1);
}
