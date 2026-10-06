import type { EventDecl, WidgetContext } from '@/types/model';
import { ValidationError, type Violation } from './data';

/**
 * Runs the microflow or nanoflow an event names on the Rust runtime.
 *
 * The same event is announced on `window` first, so code outside the widget
 * tree can observe what the user did.
 */
export async function invokeAction(
  event: EventDecl | undefined,
  context: WidgetContext,
): Promise<ActionAnswer | undefined> {
  if (!event?.handler) return undefined;
  window.dispatchEvent(new CustomEvent('mxrs:action', { detail: { ...event, context } }));
  const kind = encodeURIComponent(event.kind || 'action');
  const handler = encodeURIComponent(event.handler);
  const response = await fetch(`./api/${kind}/${handler}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ ...(event.arguments || {}), context }),
  });
  const answer = (await response.json().catch(() => null)) as {
    result?: unknown;
    message?: string;
    violations?: Violation[];
  } | null;
  if (!response.ok) {
    const message = answer?.message || `Action ${event.handler} failed (${response.status})`;
    if (answer?.violations?.length) throw new ValidationError(message, answer.violations);
    throw new Error(message);
  }
  return asAnswer(answer?.result);
}

/** What a flow answers with: its result, and what it asks the page to do. */
export type ActionAnswer = {
  result: unknown;
  effects: Array<Record<string, unknown> & { type: string }>;
};

/** The runtime's answer, whichever shape it has: a flow's result with effects, or a bare value. */
function asAnswer(value: unknown): ActionAnswer {
  if (
    typeof value === 'object' &&
    value !== null &&
    'effects' in value &&
    Array.isArray(value.effects)
  ) {
    return {
      result: (value as { result?: unknown }).result,
      effects: value.effects as ActionAnswer['effects'],
    };
  }
  return { result: value, effects: [] };
}
