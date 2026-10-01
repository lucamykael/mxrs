import { useEffect, useState } from 'react';

import type { Widget, WidgetContext } from '@/types/model';
import { text, widgetOptions } from '@/utils/widgets';

type BoundInputProps = { widget: Widget; context: WidgetContext };

/** An input bound to one attribute of the object it is rendered against. */
export function BoundInput({ widget, context }: BoundInputProps) {
  const member = text(widgetOptions(widget).attribute).split(/[./]/).at(-1) || widget.name || '';
  const [value, setValue] = useState(() => text(context?.[member]));
  useEffect(() => setValue(text(context?.[member])), [context, member]);

  if (widget.type === 'check_box') {
    return <input type="checkbox" checked={Boolean(context?.[member])} onChange={() => undefined} />;
  }
  if (widget.type === 'drop_down' || widget.type === 'combo_box') {
    return (
      <select value={value} onChange={(event) => setValue(event.target.value)}>
        <option value="">—</option>
      </select>
    );
  }
  return (
    <input
      type={widget.type === 'date_picker' ? 'date' : 'text'}
      value={value}
      onChange={(event) => setValue(event.target.value)}
    />
  );
}
