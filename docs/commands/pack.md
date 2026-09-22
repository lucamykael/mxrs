# Pack command contract

`mxrs pack FILE.mpr [--output FILE.mda] [--deployment DIR] [--force]` archives
an already-materialized Mendix deployment directory into an MDA.

`--output` defaults to `<project>/build/<name>.mda` and `--deployment` to
`<project>/deployment`, both mirroring MXRB. An existing output is preserved
unless `--force` is given.

## What this does and does not do

This archives a deployment directory that something else materialized. It
never invokes `mx` or `mxbuild`, and it never compiles anything.

That is the same scope MXRB's `pack` has. MXRB carries a `DeploymentMaterializer`
in its library, but no `bin/mxrb` command reaches it, and it works by copying
run templates out of a licensed Studio Pro installation rather than deriving
them from the model. So for both tools the deployment directory comes from
Studio Pro, and `pack` packages it.

## Archive layout

Only the five deployment roots are packaged — `model`, `web`, `native`,
`sass`, `tmp` — so build scratch beside them never reaches the archive. Empty
roots are still recorded, hidden files are included, directories are written
before files, and both are sorted by archive path.

Every entry is stamped `2000-01-01T00:00:00Z`, so packaging the same
deployment twice produces byte-identical archives. Unix permission bits are
carried through, because the Mendix runtime reads the executable bit off some
entries; the timestamp is the only thing normalized.

This is one deliberate improvement on the oracle. MXRB declares the same fixed
time, but rubyzip overwrites it with each source file's mtime when the entry is
added, so the constant never reaches the archive: `touch`-ing one unchanged file
changes MXRB's whole archive checksum. Here the fixed time is what is written,
which is what the constant was for.

The archive is written to a temporary file beside the output and renamed, so a
crash mid-write cannot leave a truncated file that still opens as a ZIP.

## Refusals

A deployment that is unmaterialized, stale, or built for another runtime
produces an MDA that is a structurally valid ZIP and a broken deployment — a
failure invisible in the artifact itself. Each case is refused up front, with
MXRB's exact message:

| Condition | Message |
| --- | --- |
| Output exists without `--force` | `<output>: file already exists` |
| `--deployment` is not a directory | `<dir>: deployment directory not found` |
| Model targets an unaudited Mendix major | `audited native compilation supports Mendix 6.x, 7.x, 9.x, 10.x, and 11.x; got <version>` |
| A required file is absent | `deployment is not materialized; missing model/model.mdp, model/metadata.json, model/bundles/project.jar, web/index.html` |
| `model/metadata.json` does not parse | `invalid model/metadata.json: <reason>` |
| `RuntimeVersion` disagrees with the model | `deployment targets Mendix <a>, but MPR targets <b>` |
| `model/model.mdp` is older than the MPR | `deployment is stale: <path> is older than <mpr>` |
| Any symlink under a deployment root | `deployment contains symlink <path>` |

A symlink is neither followed nor stored: following one pulls content from
outside the deployment into the archive, and storing one makes the archive
mean different things on different machines.

## Verification

Content parity with the oracle is the pin that matters. Verified over a real
46-file deployment, materialized by MXRB's own `DeploymentMaterializer` from a
Mendix 11.12.1 installation:

- both print `Packed 46 files for Mendix 11.12.1`;
- `mxrs mda compare` finds **0 differences**;
- inspecting the two ZIPs directly, all **72 entries** — directories included —
  match on name, permission bits, uncompressed size and CRC-32.

The whole-archive checksums differ for two reasons, both understood: rubyzip
and the Rust `zip` crate deflate differently, and MXRB stamps source mtimes
where this stamps the fixed time. Neither is a content difference, which is why
parity is asserted per entry.

Seven of the eight refusals were run against both implementations and produce
the same message text; MXRB surfaces them as uncaught Ruby exceptions with a
backtrace, while this prints `[mxrs] error: <message>` and exits non-zero. The
eighth — an unaudited Mendix major — is unit-tested instead, because forging an
MPR that declares one is harder than the check it exercises. The unparseable-
metadata message shares MXRB's `invalid model/metadata.json:` prefix; the
parser detail after it naturally differs between Ruby's JSON and `serde_json`.

```sh
cargo test -p mxrs-cli --test commands pack
mxrs pack app/App.mpr --output build/App.mda
mxrs mda compare build/App.mda build/App-from-mxrb.mda
```
