# Portable command contract

`mxrs portable FILE.mpr [--output runtime.zip] [--deployment DIR]
[--mendix-home DIR] [--force]` bundles an already-materialized Mendix
deployment together with an installed Mendix Runtime into one self-contained
`runtime.zip`: unpack it anywhere with a JVM and `bin/start` boots the
application.

`--output` defaults to `<project>/build/runtime.zip` and `--deployment` to
`<project>/deployment`, both mirroring MXRB. `--mendix-home` overrides the
Runtime tree, which otherwise comes from
`~/.local/share/mendix/<version>/runtime` for the model's Mendix version
(`runtime` is appended unless the directory named is already called
`runtime`). An existing output is preserved unless `--force` is given.

## What this does and does not do

Like `pack`, this packages what already exists: the deployment directory that
Studio Pro (or MXRB's library materializer) produced, and the Runtime
distribution that is already installed. It never invokes `mx`/`mxbuild` and
never compiles anything.

The same validations as `pack` run first, with the same messages: the
deployment must exist, be materialized (the four required files are named
when missing), be fresh relative to the `.mpr`, and target the model's
Mendix version. On top of those, the Runtime tree must carry
`launcher/runtimelauncher.jar` — a tree without it is refused as
`Mendix Runtime is incomplete at ...; missing ...`.

## Archive layout

MXRB's `PortableArchiveWriter` layout, entry for entry:

- `lib/runtime/` — the whole Runtime tree, hidden files included, file modes
  carried through, inner directories normalized to `0755`.
- `app/<root>/` — the deployment roots `model web native sass tmp` plus
  `run`. `data/` and `log/` are **not** copied: local state must not ship.
- `app/data/`, `app/data/database/`, `app/data/files/`, `app/data/tmp/`,
  `app/log/` — created empty for the Runtime's first boot.
- `bin/` — start scripts rendered from the Runtime's own `pad/bin/*.hbs`
  templates (UTF-8 BOM stripped, `{{!-- ... --}}` comments removed,
  `{{DefaultConfig}}` resolved to `Default`); only `start` is executable. A
  distribution without PAD templates gets MXRB's fallback POSIX `bin/start`.
- `etc/` — `example.conf` and `variables.conf` from the Runtime's `pad/etc`
  when it ships them (fallbacks otherwise), plus five HOCON files rendered
  from the deployment's `model/metadata.json`: `Default` (the include list),
  `StudioPro.conf` (scheduled events, `BCRYPT:12`, a debugger password line
  for Runtime 11+), `configurations/Default.conf` (HSQLDB defaults),
  `constants/defaults.conf` (each constant's model default) and
  `constants/variables.conf` (each constant as a `${?CONSTANTS_...}`
  environment override).

Symlinks anywhere in the inputs are refused rather than followed or stored,
for the same reason `pack` refuses them.

## Determinism

Every entry is stamped `2000-01-01T00:00:00Z`, so bundling the same inputs
twice produces byte-identical archives. As with `pack`, this is a deliberate
improvement on the oracle: MXRB declares the same fixed time but rubyzip
overwrites it with each source file's mtime, so touching one unchanged
Runtime file changes MXRB's whole archive checksum.

The archive is written to a temporary file beside the output and renamed, so
a crash mid-write cannot leave a truncated file that still opens as a ZIP.

## Verification

The five rendered configuration files and the three fallback assets are
pinned byte-for-byte against MXRB's own rendering
(`tests/fixtures/portable_config_oracle.json`, dumped from
`PortableConfiguration`/`PortableFallbackAssets`). Live, on the real Mendix
11.12.1 Runtime and a deployment materialized by MXRB from the `minimal`
fixture, `mxrb portable` and `mxrs portable` produced archives with the same
3993 files (4353 entries) and identical per-entry name, mode, size and CRC —
the only difference being MXRB's non-deterministic timestamps.
