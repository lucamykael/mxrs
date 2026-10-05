import { createContext, type ReactNode } from 'react';

import type { Form } from '@/mxrs/forms';
import type { NavigationItem } from '@/types/model';

/** The title of the page being drawn. */
export const PageTitle = createContext('');

/** What a layout's placeholders hold for the page being drawn, by name. */
export const Placeholders = createContext<ReadonlyMap<string, ReactNode>>(new Map());

/**
 * What the application gives every page: its menu, a way to open another
 * page, and the layout or snippet a qualified name names.
 */
export const Shell = createContext<{
  items: NavigationItem[];
  open: (page: string) => void;
  form: (qualified: string) => Form | undefined;
}>({
  items: [],
  open: () => {},
  form: () => undefined,
});
