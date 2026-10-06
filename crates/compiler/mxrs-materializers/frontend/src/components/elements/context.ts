import { createContext, type ReactNode } from 'react';

import type { DataObject } from '@/api/data';
import type { Form } from '@/mxrs/forms';
import type { Collections, NavigationItem } from '@/types/model';

/** The object the widgets being drawn are about: a row of a list, or a form's. */
export const Row = createContext<DataObject | null>(null);

/** The form the inputs being drawn fill: what it holds now, and how to change it. */
export const Draft = createContext<{
  object: DataObject | null;
  /** The members the user changed, which are what a save sends. */
  changed: ReadonlySet<string>;
  set: (member: string, value: unknown) => void;
  /** Takes the object as the runtime saved it. */
  saved: (object: DataObject) => void;
} | null>(null);

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
  /** Opens a page, giving it the objects its parameters take. */
  open: (page: string, given?: Record<string, DataObject>) => void;
  /** What the page being shown was given. */
  given: Record<string, DataObject>;
  /** Counts what changed in the data, so what shows it reads it again. */
  changes: number;
  changed: () => void;
  /** Says what went wrong to the user. */
  fail: (error: unknown) => void;
  form: (qualified: string) => Form | undefined;
  /** The icon collections' classes and the images' files, by qualified name. */
  collections: Collections;
  /** Whether the user opened or closed the sidebar, which every page of the layout keeps. */
  sidebar?: boolean;
  setSidebar: (open: boolean) => void;
}>({
  setSidebar: () => {},
  collections: { icons: {}, images: {} },
  items: [],
  current: '',
  open: () => {},
  given: {},
  changes: 0,
  changed: () => {},
  fail: () => {},
  form: () => undefined,
});
