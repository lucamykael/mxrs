/** An object of the model, as the runtime shows it. */
export type DataObject = {
  entity: string;
  id: string;
  members: Record<string, unknown>;
};

/**
 * What a page does with the objects it shows: `retrieve` those of an
 * entity, `create` a blank one, `save` what a form holds, `delete` one.
 */
export async function data<T>(operation: string, body: Record<string, unknown>): Promise<T> {
  const response = await fetch(`./api/data/${operation}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  const answer = (await response.json().catch(() => null)) as {
    result?: T;
    message?: string;
  } | null;
  if (!response.ok || !answer) {
    throw new Error(answer?.message || `${operation} failed (${response.status})`);
  }
  return answer.result as T;
}
