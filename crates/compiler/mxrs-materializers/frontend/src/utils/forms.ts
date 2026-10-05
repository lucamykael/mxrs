import type { ReactElement } from 'react';

import type { Form } from '@/mxrs/forms';

type Declared = { default?: Form };

let declared: Map<string, Form> | undefined;

/** Every page, layout and snippet the frontend declares, by qualified name. */
function forms(): Map<string, Form> {
  if (declared) return declared;
  const files = import.meta.glob<Declared>(
    ['../pages/*/*.tsx', '../components/layout/*/*.tsx', '../components/snippets/*/*.tsx'],
    { eager: true },
  );
  declared = new Map();
  for (const file of Object.values(files)) {
    const form = file.default;
    const name = (form?.document as ReactElement<{ name?: string }> | undefined)?.props.name;
    if (form && name) declared.set(`${form.module}.${name}`, form);
  }
  return declared;
}

/** The form of that qualified name, when the frontend declares it. */
export const findForm = (qualified: string): Form | undefined => forms().get(qualified);
