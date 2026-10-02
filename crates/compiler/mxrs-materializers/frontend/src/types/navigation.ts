// The shape of the project's navigation. `src/navigation/index.ts` declares
// it; mxrs reads that file into the model on every build.

/** A text in each language the application speaks, by language code. */
export type Localized = Record<string, string>;

export interface NavigationItem {
  /** The item's text: English alone, or by language. */
  caption: string | Localized;
  /** The page the item opens, as `Module.Page`. */
  page?: string;
  /** The microflow the item calls, as `Module.Flow`. */
  microflow?: string;
  icon?: { glyph: string } | { code: number };
  items?: NavigationItem[];
}

/** The home of a role whose home is not its profile's: a page or a microflow. */
export interface RoleHome {
  role: string;
  page?: string;
  microflow?: string;
}

export interface NavigationProfile {
  name: string;
  /** `Responsive` unless stated. */
  kind?: string;
  title?: Localized;
  homePage?: string;
  homeMicroflow?: string;
  signInPage?: string;
  homes?: RoleHome[];
  items?: NavigationItem[];
}

export interface Navigation {
  profiles: NavigationProfile[];
}
