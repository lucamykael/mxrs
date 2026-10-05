/** A value the runtime sends or the model declares: JSON, as it travels. */
export type RuntimeValue =
  | string
  | number
  | boolean
  | null
  | RuntimeValue[]
  | Record<string, unknown>;

/** What a widget does when the user acts on it. */
export type EventDecl = {
  event?: string;
  kind?: string;
  handler?: string;
  arguments?: Record<string, RuntimeValue>;
};

export type Widget = {
  type: string;
  name?: string;
  options?: Record<string, RuntimeValue>;
  events?: EventDecl[];
  children?: Widget[];
};

export type Page = {
  name: string;
  qualified_name: string;
  title?: string;
  widgets: Widget[];
};

export type NavigationItem = {
  caption?: string;
  page?: string | null;
  microflow?: string | null;
  /** A glyph of the theme's icon font, by name. */
  icon?: string | number | null;
  items?: NavigationItem[];
};

export type NavigationProfile = {
  name: string;
  home_page?: string;
  items: NavigationItem[];
};

/** `model.json`: the application as the Rust runtime publishes it. */
export type Manifest = {
  format: number;
  project: { name: string; mendix_version: string };
  modules: Array<{ name: string; pages: Page[] }>;
  navigation: { profiles: NavigationProfile[] };
};

/** The object a widget is rendered against, when it has one. */
export type WidgetContext = Record<string, RuntimeValue> | null;
