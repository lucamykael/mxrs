import type { EventDecl, WidgetContext } from '@/types/model';

/**
 * Runs the microflow or nanoflow an event names on the Rust runtime.
 *
 * The same event is announced on `window` first, so code outside the widget
 * tree can observe what the user did.
 */
export async function invokeAction(event: EventDecl | undefined, context: WidgetContext) {
  if (!event?.handler) return;
  window.dispatchEvent(new CustomEvent('mxrs:action', { detail: { ...event, context } }));
  const kind = encodeURIComponent(event.kind || 'action');
  const handler = encodeURIComponent(event.handler);
  const response = await fetch(`./api/${kind}/${handler}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ ...(event.arguments || {}), context }),
  });
  if (!response.ok) throw new Error(`Action ${event.handler} failed (${response.status})`);
}
