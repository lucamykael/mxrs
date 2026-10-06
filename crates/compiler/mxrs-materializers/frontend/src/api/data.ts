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

/** One thing wrong with what a form saved: the member and what the model says of it. */
export type Violation = { member: string; message: string };

/** A save the runtime refused for the model's rules: what to show under which inputs. */
export class ValidationError extends Error {
  constructor(
    message: string,
    readonly violations: Violation[],
  ) {
    super(message);
  }
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
    violations?: Violation[];
  } | null;
  if (!response.ok || !answer) {
    const message = answer?.message || `${handler} failed (${response.status})`;
    if (answer?.violations?.length) throw new ValidationError(message, answer.violations);
    throw new Error(message);
  }
  return answer.result as T;
}

/** How a list asks for its objects: within an XPath constraint, in a sort order, a page of them. */
export type Query = {
  constraint?: string;
  sort?: { attribute: string; descending: boolean }[];
  offset?: number;
  limit?: number;
};

/** The objects of an entity the runtime holds, as the query says, and how many there are in all. */
export async function retrieve(
  entity: string,
  query: Query = {},
): Promise<{ objects: DataObject[]; total: number }> {
  const answer = await data<{ objects: DataObject[]; total?: number }>('retrieve', {
    entity,
    ...query,
  });
  return { objects: answer.objects, total: answer.total ?? answer.objects.length };
}

/** Whether what the runtime answered is an object of the model. */
export const isObject = (value: unknown): value is DataObject =>
  typeof value === 'object' &&
  value !== null &&
  typeof (value as DataObject).entity === 'string' &&
  typeof (value as DataObject).members === 'object';
