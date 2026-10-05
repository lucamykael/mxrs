// What a design property means. A theme's widgets are mostly styled by the
// design properties a page states for them — "Flex container: Horizontal
// (row)", "Spacing: S" — and each module says, in its
// `design-properties.json`, which class or variable an option stands for.
import properties from 'virtual:mxrs-design-properties';

import { child, list, plain, sourceOf, type Source } from './view';

type Sides = Partial<Record<'top' | 'right' | 'bottom' | 'left', { class?: string }>>;
type Option = { name: string; class?: string; variable?: string; oldNames?: string[] };
type Property = {
  name: string;
  oldNames?: string[];
  type: string;
  class?: string;
  property?: string;
  options?: Option[];
  margin?: (Sides & { name: string })[];
  padding?: (Sides & { name: string })[];
};

const declared = properties as Record<string, Property[]>;

/** The key a module's design properties are declared under, for a stored type. */
const KEYS: Record<string, string> = {
  ActionButton: 'Button',
  LinkButton: 'Button',
  SidebarToggleButton: 'Button',
  TabControl: 'TabContainer',
};

/** What an element's design properties add to it: classes, and variables as a style. */
export type Design = { classes: string[]; style: Record<string, string> };

const called = (named: { name: string; oldNames?: string[] }, name: string) =>
  named.name === name || (named.oldNames?.includes(name) ?? false);

/**
 * The classes and variables the design properties of `appearance` stand
 * for, on an element of the stored `type` (or the pluggable widget
 * `widget`). A property no module declares, or an option it does not have,
 * adds nothing.
 */
export function design(type: string, appearance: Source | null, widget?: string): Design {
  const found: Design = { classes: [], style: {} };
  if (!appearance) return found;
  const short = type.split('$').pop() ?? type;
  const known = [
    ...(declared[widget ?? ''] ?? []),
    ...(declared[KEYS[short] ?? short] ?? []),
    ...(declared.Widget ?? []),
  ];
  for (const item of list(appearance, 'designProperties')) {
    const stated = sourceOf(item);
    if (!stated) continue;
    const property = known.find((candidate) => called(candidate, plain(stated, 'Key', '')));
    const value = child(stated, 'value');
    if (!property || !value) continue;
    if (value.type === 'Forms$ToggleDesignPropertyValue') {
      if (property.class) found.classes.push(property.class);
    } else if (value.type === 'Forms$CompoundDesignPropertyValue') {
      // Spacing: a size per side, stated as `margin-top`, `padding-left`, ...
      for (const part of list(value, 'properties')) {
        const side = sourceOf(part);
        const size = side ? child(side, 'value') : null;
        if (!side || !size) continue;
        const [group, where] = plain(side, 'Key', '').split('-') as [
          'margin' | 'padding',
          keyof Sides,
        ];
        const chosen = property[group]?.find((option) => option.name === plain(size, 'option', ''));
        const name = chosen?.[where]?.class;
        if (name) found.classes.push(name);
      }
    } else {
      // One option, or several separated the way Studio Pro stores them.
      for (const name of plain(value, 'option', '').split(/\s*[,;]\s*/)) {
        const option = property.options?.find((candidate) => called(candidate, name));
        if (option?.class) found.classes.push(option.class);
        if (option?.variable && property.property) {
          found.style[property.property] = `var(${option.variable})`;
        }
      }
    }
  }
  return found;
}
