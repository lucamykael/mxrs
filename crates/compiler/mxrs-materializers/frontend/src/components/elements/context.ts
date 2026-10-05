import { createContext, type ReactNode } from 'react';

import type { Form } from '@/mxrs/forms';
import type { NavigationItem } from '@/types/model';

/** The title of the page being drawn. */
export const PageTitle = createContext('');

/** Whether what is being drawn is a page's own content, which a theme styles as `mx-page`. */
export const OfPage = createContext(false);

/** The sidebar of the scroll container being drawn: whether it is open, and its toggle. */
export const Sidebar = createContext<{ open: boolean; toggle: () => void }>({
  open: true,
  toggle: () => {},
});

/** What a layout's placeholders hold for the page being drawn, by name. */
export const Placeholders = createContext<ReadonlyMap<string, ReactNode>>(new Map());

/**
 * What the application gives every page: its menu, a way to open another
 * page, and the layout or snippet a qualified name names.
 */
export const Shell = createContext<{
  items: NavigationItem[];
  /** The page being shown, by qualified name. */
  current: string;
  open: (page: string) => void;
  form: (qualified: string) => Form | undefined;
  /** Whether the user opened or closed the sidebar, which every page of the layout keeps. */
  sidebar?: boolean;
  setSidebar: (open: boolean) => void;
}>({
  setSidebar: () => {},
  items: [],
  current: '',
  open: () => {},
  form: () => undefined,
});
