type PageNotFoundProps = { requested: string };

export function PageNotFound({ requested }: PageNotFoundProps) {
  return <p role="alert">Page not found: {requested}</p>;
}
