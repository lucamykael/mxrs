import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from '@/App';
import '@/styles/index.css';

// The project's theme, as the model's own tooling compiled it: what the
// classes its pages state mean. A project without one keeps the shell's
// plain styles, which every rule of a theme takes precedence over.
import.meta.glob('../../assets/theme-cache/web/theme.compiled.css', { eager: true });

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
