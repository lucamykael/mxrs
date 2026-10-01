import { useEffect, useState } from 'react';

import { fetchManifest } from '@/api/model';
import type { Manifest } from '@/types/model';

/** The application model, once loaded — or why it could not be. */
export function useManifest() {
  const [manifest, setManifest] = useState<Manifest | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    fetchManifest()
      .then(setManifest)
      .catch((error: unknown) => setFailure(error instanceof Error ? error.message : String(error)));
  }, []);

  return { manifest, failure };
}
