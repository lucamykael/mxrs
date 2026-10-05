/// <reference types="vite/client" />

/** Every module's `design-properties.json`, merged by the key they declare properties under. */
declare module 'virtual:mxrs-design-properties' {
  const properties: Record<string, unknown[]>;
  export default properties;
}
