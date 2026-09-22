# Pack command contract

`mxrs pack FILE.mpr [--output FILE.mda] [--deployment DIR] [--force]` archives
an already-materialized Mendix deployment directory into an MDA.

`--output` defaults to `<project>/build/<name>.mda` and `--deployment` to
`<project>/deployment`, both mirroring MXRB. An existing output is preserved
unless `--force` is given.

## What this does and does not do

MXRB's `pack` is two halves. This is the container half — a port of
`compiler/packager.rb` — and it stops exactly where that file does: it never
invokes `mx` or `mxbuild`, and it never compiles anything.

Materializing `deployment/` **from the model** is the other half: page and
widget bundle compilation, Java proxy generation, the project jar, the web
shell. That is roughly 12k lines of `lib/mxrb/compiler/` and is **not ported**.
Until it is, the deployment directory has to come from MXRB or from Studio Pro;
`mxrs pack` then packages it.

## Archive layout

Only the five deployment roots are packaged — `model`, `web`, `native`,
`sass`, `tmp` — so build scratch beside them never reaches the archive. Empty
roots are still recorded, hidden files are included, directories are written
before files, and both are sorted by archive path.

Every entry is stamped `2000-01-01T00:00:00Z`, so packaging the same
deployment twice produces byte-identical archives. Unix permission bits are
carried through, because the Mendix runtime reads the executable bit off some
entries; the timestamp is the only thing normalized.

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

Content parity with the oracle is the pin that matters. Over the same
deployment directory, `mxrb pack` and `mxrs pack` report the same file count
and Mendix version, and `mxrs mda compare` finds **0 differences** between the
two archives — same entry set, same SHA-256 per entry. The whole-archive
checksums differ because rubyzip and the Rust `zip` crate deflate differently;
that is a compression artifact, not a content one, which is why comparison is
done at the entry level.

All eight refusals above were run against both implementations and produce the
same message text. MXRB surfaces them as uncaught Ruby exceptions with a
backtrace; this prints `[mxrs] error: <message>` and exits non-zero.

```sh
cargo test -p mxrs-cli --test commands pack
mxrs pack app/App.mpr --output build/App.mda
mxrs mda compare build/App.mda build/App-from-mxrb.mda
```
