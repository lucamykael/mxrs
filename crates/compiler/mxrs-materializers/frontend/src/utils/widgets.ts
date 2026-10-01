import type { CSSProperties } from 'react';

import type { RuntimeValue, Widget } from '@/types/model';

export const widgetOptions = (widget: Widget) => widget.options || {};

export const text = (value: RuntimeValue | undefined) => (value == null ? '' : String(value));

export const widgetCaption = (widget: Widget) =>
  text(widgetOptions(widget).caption || widget.name || widget.type);

/** A stable key for a widget among its siblings. */
export const widgetKey = (widget: Widget, index: number) =>
  `${widget.name || widget.type}-${index}`;

/** The model's inline `style` text as the object React expects. */
export const widgetStyle = (widget: Widget): CSSProperties | undefined => {
  const source = widgetOptions(widget).style;
  if (typeof source !== 'string' || !source.trim()) return undefined;
  return Object.fromEntries(
    source.split(';').flatMap((entry) => {
      const [property, value] = entry.split(':', 2).map((part) => part.trim());
      if (!property || !value) return [];
      return [[property.replace(/-([a-z])/g, (_match, letter: string) => letter.toUpperCase()), value]];
    }),
  );
};
