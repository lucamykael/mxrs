import type { Manifest } from '@/types/model';

/** Loads the application model the Rust runtime serves beside the bundle. */
export async function fetchManifest(): Promise<Manifest> {
  const response = await fetch('./model.json');
  if (!response.ok) throw new Error(`model.json failed (${response.status})`);
  return response.json() as Promise<Manifest>;
}
