import { useEffect, useState } from 'react';

const currentRoute = () => decodeURIComponent(location.hash.slice(1));

/** The page named by the URL hash, and a way to open another. */
export function useHashRoute() {
  const [route, setRoute] = useState(currentRoute);

  useEffect(() => {
    const changed = () => setRoute(currentRoute());
    addEventListener('hashchange', changed);
    return () => removeEventListener('hashchange', changed);
  }, []);

  const open = (page: string) => {
    location.hash = encodeURIComponent(page);
  };

  return [route, open] as const;
}
