# ER-diagram editor

The browser editor `mxrs diagram-er` serves is the build of mxrb's
`frontend/modeler/src/domain-diagram` (mxrb `lib/mxrb/web_ui`, origin/main
563c75b), MIT License, Copyright (c) 2026 Lucas Moura.
Only `domain.html`'s title is changed. The editor speaks mxrb's protocol:
`GET /api/diagram`, and `POST /api/layout` with its `X-MXRB-Token` header.
