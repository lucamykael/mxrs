// The shape of the project's navigation. `src/navigation/index.ts` declares
// it; mxrs reads that file into the model on every build.

/** A text in each language the application speaks, by language code. */
export type Localized = Record<string, string>;

/** What a menu item does: open a page, call a microflow, or nothing. */
type ItemTarget =
  | { /** The page the item opens, as `Module.Page`. */ page: string; microflow?: never }
  | { /** The microflow the item calls, as `Module.Flow`. */ microflow: string; page?: never }
  | { page?: never; microflow?: never };

/** A glyph by its name, or by its character code (a whole number). */
export type NavigationIcon = { glyph: string; code?: never } | { code: number; glyph?: never };

export type NavigationItem = ItemTarget & {
  /** The item's text: English alone, or by language. */
  caption: string | Localized;
  icon?: NavigationIcon;
  items?: NavigationItem[];
};

/** The home of a role whose home is not its profile's: a page or a microflow. */
export type RoleHome = { role: string } & (
  | { page: string; microflow?: never }
  | { microflow: string; page?: never }
);

/** Where a profile starts: a page, a microflow, or neither. */
type ProfileHome =
  | { homePage: string; homeMicroflow?: never }
  | { homeMicroflow: string; homePage?: never }
  | { homePage?: never; homeMicroflow?: never };

export type NavigationProfile = ProfileHome & {
  /** Unique among the profiles. */
  name: string;
  /** `Responsive` unless stated. */
  kind?: string;
  title?: Localized;
  signInPage?: string;
  homes?: RoleHome[];
  items?: NavigationItem[];
};

export interface Navigation {
  profiles: NavigationProfile[];
}
