import { useEffect, useState } from 'react';

import { fetchCollections, fetchManifest } from '@/api/model';
import type { Collections, Manifest } from '@/types/model';

/** The application model, once loaded — or why it could not be. */
export function useManifest() {
  const [manifest, setManifest] = useState<Manifest | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    fetchManifest()
      .then(setManifest)
      .catch((error: unknown) =>
        setFailure(error instanceof Error ? error.message : String(error)),
      );
  }, []);

  return { manifest, failure };
}

const none: Collections = { icons: {}, images: {} };

/** The model's collections, once loaded; none until then. */
export function useCollections(): Collections {
  const [collections, setCollections] = useState<Collections>(none);
  useEffect(() => {
    fetchCollections().then(setCollections);
  }, []);
  return collections;
}
