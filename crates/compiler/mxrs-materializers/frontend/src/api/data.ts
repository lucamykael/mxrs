/** An object of the model, as the runtime shows it. */
export type DataObject = {
  entity: string;
  id: string;
  members: Record<string, unknown>;
  /** A blank object a form was given: it exists once it is saved. */
  new?: boolean;
};

/**
 * What a page does with the objects it shows: `retrieve` those of an
 * entity, `create` a blank one, `save` what a form holds, `delete` one.
 */
export async function data<T>(operation: string, body: Record<string, unknown>): Promise<T> {
  return invoke<T>('data', operation, body);
}

/** Asks the runtime for something by kind and name: a data operation, a microflow, a nanoflow. */
export async function invoke<T>(
  kind: string,
  handler: string,
  body: Record<string, unknown>,
): Promise<T> {
  const response = await fetch(`./api/${encodeURIComponent(kind)}/${encodeURIComponent(handler)}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const answer = (await response.json().catch(() => null)) as {
    result?: T;
    message?: string;
  } | null;
  if (!response.ok || !answer) {
    throw new Error(answer?.message || `${handler} failed (${response.status})`);
  }
  return answer.result as T;
}

/** Whether what the runtime answered is an object of the model. */
export const isObject = (value: unknown): value is DataObject =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as DataObject).entity === 'string' &&
  typeof (value as DataObject).members === 'object';
