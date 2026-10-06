import type { Collections, Manifest } from '@/types/model';

/** Loads the application model the Rust runtime serves beside the bundle. */
export async function fetchManifest(): Promise<Manifest> {
  const response = await fetch('./model.json');
  if (!response.ok) throw new Error(`model.json failed (${response.status})`);
  return response.json() as Promise<Manifest>;
}

/** The model's collections, when the build wrote them; none otherwise. */
export async function fetchCollections(): Promise<Collections> {
  const response = await fetch('./collections.json').catch(() => null);
  if (!response?.ok) return { icons: {}, images: {} };
  const read = (await response.json().catch(() => null)) as Partial<Collections> | null;
  return { icons: read?.icons ?? {}, images: read?.images ?? {} };
}
