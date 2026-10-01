# Verification and remaining parity work

Passing tests, covering source lines, compiling a model, running that model,
and matching Studio Pro are different claims. None substitutes for the others.
The target is complete behavior and exact 100% coverage, not a rounded score.

> **Policy amendment (2026-09-21, user directive).** mxrs owns its runtime
> and compiler. A capability-matrix row is now Verified either through an
> executable mxrb-comparison oracle or through OWN behavioral evidence
> proving full capability where an mxrb comparison is impossible by
> construction (Rust-native product surfaces, the MXRS-owned runtime and its
> interpreter, scheduler, and relational persistence). "Partial by
> construction" is no longer a resting state: every Partial row names the
> concrete capability that is still missing. The executable matrix
> (`cargo run -p xtask -- capability-matrix`) is authoritative; prose below
> this line predates the amendment where it says a row "stays partial"
> solely for lack of an mxrb oracle.

## Reproducible local gates

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p xtask -- oracle-diff xtask/fixtures/minimal
cargo run -p xtask -- oracle-diff xtask/fixtures/with_page
cargo run -p xtask -- noise-audit
cargo run -p xtask -- capability-matrix --check-baseline
bash xtask/coverage.sh target/coverage.json
cargo run -p xtask -- coverage-gate target/coverage.json
```

The coverage driver allocates a fresh `mktemp` directory for every run, with
separate outer/nested Cargo targets. Use `MXRS_COVERAGE_TMPDIR="$PWD/target"`
when `/tmp` is space-constrained; CI defaults to `RUNNER_TEMP`. Keep source
unchanged until measurement finishes. All mapped ELF objects—including generated
application binaries, doctests and build scripts—are passed to the report;
profiles, toolchain versions and source/object SHA-256 inventories are retained.

`clean --workspace` alone does not guarantee fresh coverage maps across Cargo
layouts/toolchains. The 2026-09-13 diagnosis found an obsolete Rust 1.95 test ELF
under `debug/deps` alongside Rust 1.100-nightly artifacts under `debug/build`.
The obsolete runtime map added exactly 449 unexecuted lines (989 versus 540),
so that mixed-artifact report is not valid evidence of current coverage. No
threshold was reduced and no source file was excluded to address the issue.

The HTTP integration tests open real local TCP listeners. A sandbox denying
socket creation must not turn those failures into ignored tests: run the same
tests with the required local-network permission.

`noise-audit` covers the full Rust tree produced by `mxrs import` for every
committed fixture and a generated project containing every public artifact
scaffold. UUIDs, storage hashes/schema keys, raw unit/container identifiers,
TODOs, and opacity markers fail the gate. Lossless import metadata remains
under `model/imported/`; it is not duplicated into developer-facing Rust.

The following readiness gates intentionally fail while work remains. They do
not accept source exclusions, missing instrumentation, truncated inventories,
rounded percentages, or a partial command implementation as complete:

```sh
cargo run -p xtask -- capability-matrix --check-baseline --require-complete
cargo run -p xtask -- coverage-gate target/coverage.json --require-complete
```

The command inventory currently covers MXRB's 76 top-level commands, not an
exhaustive list of Studio Pro capabilities. All 76 still need complete
command-contract evidence: 58 have partial equivalents and 18 are absent.

## Runtime and deployment command increment

The ten runtime/deployment rows now have deliberately partial MXRS surfaces:

- `mda inspect/compare` safely inventories ZIP paths, requires Mendix
  `model/metadata.json`, and compares content hashes. It does not build or run
  an MDA.
- `migrate check/plan` rebuilds a Cargo-native project offline and compares it
  with the retained lossless import snapshot. It is a drift check, not a
  Studio Pro database migration engine.
- `preflight` audits storage plus the native page/layout, flow/code-action and
  nanoflow compilers. Unsupported behavior is reported; it is not silently
  treated as compatible.
- `env` layers dotenv profiles as data, with process variables winning, and
  reports keys and source files without printing values. It does not evaluate
  shell syntax or interpolation.
- `cache status/warm/clear` manages an external semantic-index cache keyed by
  the absolute MPR path and invalidated by a native unit-content fingerprint.
  Semantic CLI queries consume valid warmed entries; cache files never modify
  the MPR.
- `doctor` checks a Cargo-native project, its generated MPR when present, and
  the local Cargo/Rust/Java/Docker executables. Optional Java/Docker absence is
  a warning rather than a false project error.
- `repository new` generates a transactional application port and
  infrastructure adapter. It is Rust source and therefore cannot be compared
  byte-for-byte with MXRB's Ruby generator.
- `upgrade` previews by default and transactionally migrates pre-layered
  generated projects to the four-layer composition root. `--mendix VERSION`
  additionally changes only generated version markers; without it, the model
  version stays unchanged. Existing domain source is preserved, partial or
  hand-shaped layouts fail closed, and writes happen only under `--apply`.
  It does not run Studio Pro's metamodel conversions.
- `project inspect` reports the generated source layout as `pre-layered`,
  `layered`, or `incomplete`, so migration failures have a machine-readable
  diagnosis instead of requiring the developer to infer it from missing
  directories.
- `db status/up/down/destroy/credentials/url` manages an isolated PostgreSQL
  13 container and volume. Resources are loopback-only, project-keyed, labeled,
  and ownership-checked before mutation; state and the generated password live
  outside the project in mode-0600 files, and the password is passed to Docker
  through a mounted file rather than a command-line value. A real local smoke
  test on 2026-09-14 completed `up -> status -> down -> destroy` and left no
  container or volume. Mendix Runtime boot, schema synchronization, SQL/shell,
  plan/workload/index analysis, and stale-runtime replacement are not ported.
- `team-server login/status` stores only a pointer to a user-managed PAT file
  and validates local repositories against the official HTTPS host before
  showing Git status. Projects/info/branches/commits APIs and clone/fetch/pull/
  push are not ported, so this increment makes no Team Server network request.

These rows are `Partial`, not `Verified`: the observable contracts are smaller
than MXRB's, and no command-level differential oracle covers them.

`coverage-gate` prints exact covered, total, and uncovered counts for every
metric. Record those counts with their commit; do not carry a percentage into a
new source revision. Passing the development gate does not pass the exact-100%
readiness gate.

MXRB's model-authoring generators write Ruby source into a project; the MXRS
equivalents write Rust declarations into `src/domain/modules/`. Because the
generated content differs by construction, no differential oracle can compare
it, and those rows stay partial rather than verified however many tests pass.
What is proven is narrower and stated as such: a project scaffolded with
`module`/`entity`/`enumeration`/`constant`/`scheduled-event`/`use-case`/
`nanoflow`/`page`/`published-rest`/`consumed-rest`/`java-action`/`security`
compiles with real Cargo, runs, and the declarations reach the written `.mpr`;
the argument grammar, rendering, dry-run and registry behavior are covered by
CLI tests. Nothing here establishes that Studio Pro accepts the result.

`constant` and `scheduled-event` were previously absent because `mxrs-ir` had
no `ConstantDecl`/`ScheduledEventDecl` for their templates to compile against.
That surface now exists end to end — IR declarations, DSL builders, and
`mxrs-writer` lowering that persists `Constants$Constant` and
`ScheduledEvents$ScheduledEvent` — and both commands are registered. Two
limits are deliberate and worth naming rather than discovering later:

- The schedule vocabulary is `minutes`/`hours`/`days`. MXRB names eight
  `IntervalType` values, but its own `scheduled_event_schedule_doc` raises for
  the other five, so offering them would emit documents the oracle refuses to
  build. Widening this needs oracle evidence, not a larger enum.
- `OnOverlap` accepts the two values attested in MXRB's fixtures (`SkipNext`,
  `DelayNext`). MXRB keeps the field an open string; typing it here means an
  unattested value cannot reach the `.mpr` silently.

Neither command is verified: the readback is MXRS reading its own writes, and
no executable oracle yet compares either command's contract against MXRB's.

`mxrs page` now carries MXRB's template catalog (`page templates`, the same
four audited patterns) and its `--template`/`--chain` vertical slices. What is
proven is that every catalogued template compiles on its own, and that a
`page:nanoflow:microflow` chain produces a slice — backing entity, loader,
refresh microflow, refresh nanoflow and the page — that compiles with real
Cargo and whose artifacts all reach the written `.mpr`. Two gaps are
deliberate:

- MXRB's refresh microflow logs a message and its refresh nanoflow shows one.
  `mxrs_ir::Activity` has neither activity, so those bodies are generated
  empty rather than filled with an unrelated activity.
- MXRB's `number_input` has no MXRS widget; a decimal attribute binds to a
  `text_box`, which is what Mendix itself renders for one.

The slice names its model the way any authored code does: the entity's
accessors (`OrderOverview::total()`) and the types its flows' own declarations
generate (`ACT_LoadOrderOverview`), imported from the files that declare them.
The page also adds itself to the Responsive navigation, from its own file.

Entity access rules now have an authoring surface
(`EntityBuilder::access_rule`, with members named by attribute/association
marker) and are persisted as `DomainModels$AccessRule`. This closed a
write-side loss, not only a missing feature: `Entity::to_bson` previously wrote
an empty `accessRules` array unconditionally, so an entity read with rules and
written back lost its security. Rules are three-state — an entity that declares
none keeps its imported rules verbatim, and clearing them requires saying so —
and rule/member identities derive from the role set and member reference rather
than array position, so reordering declarations does not renumber them. Not
covered: access rules are not re-exported into `project! {}` source (the same
gap indexes and lifecycle handlers have), and role references are not validated
against declared module roles, matching MXRB.

## Official fresh-project acceptance

On 2026-09-13, a new application authored with the current Rust scaffold and
built through `cargo-mxrs` passed MxBuild 11.12.1 with process exit 0 and
`BUILD SUCCEEDED`, including the Java build and package creation. The fresh
output contained a 313,068-byte `model.mdp` (34 documents, 14 types) and a
598,923-byte MDA (118 entries). The official English translations contained
the Home title and welcome text.

The official loader exposed defects that our own readback tests did not:
the root GUID's BSON representation, missing identities inside security
objects, the V2 filename/transaction contract, the legacy Java-version
conversion and empty language codes on new page text. Each now has a
regression test. The Java compatibility correction is scoped to the audited
11.12.1 fresh-project baseline; imported settings are not rewritten by it.

This proves one new scaffold can be built by the official tool, not that
every supported declaration, widget, platform or Mendix version is compatible.
Always use a new output directory: an existing `model.mdp` is not evidence
that the current run generated a valid model. External corpus artifacts are
never overwritten to run this check.

## Unversioned-model acceptance

The six corpus tests are ignored by the portable suite because their authorized
inputs cannot be committed. Invoking them explicitly without every configured
prerequisite fails; it is never a successful skip. Paths are supplied as data,
so the repository records no project name, private directory layout or
corpus-specific baseline. `MXRS_ACCEPTANCE_PROJECTS` and
`MXRS_ACCEPTANCE_MODEL_PACKAGES` are platform path lists.

```sh
export MXRS_ACCEPTANCE_PROJECTS="/authorized/a.mpr:/authorized/b.mpr"
export MXRS_ACCEPTANCE_MODEL_PACKAGES="/authorized/a.mdp:/authorized/b.mdp"
export MXRS_LOOP_ORACLE_PROJECT="/authorized/loop-source.mpr"
export MXRS_LOOP_ORACLE_MODEL="/authorized/loop-runtime.mdp"
export MXRS_PAGE_BUNDLE_ORACLE_MPR="/authorized/pages.mpr"
export MXRS_MODEL_PACKAGE_ORACLE="/authorized/model.mdp"
cargo test --no-fail-fast \
  -p mxrs-compiler-flow -p mxrs-compiler-widgets -p mxrs-pluggable -p mxrs-schema \
  --test acceptance_corpus --test loop_oracle --test page_bundle_oracle \
  --test instance_oracle --test schema_oracle --test model_package_oracle \
  -- --ignored --nocapture
```

The harness checks flow/code-action/nanoflow compilation, widget schemas and
instances, page bundles, loop structure against official compiler output and
byte-identical model-package rewriting. It reports counts only to the local
invocation and fails on incomplete evidence. These checks do not prove browser
behavior or equivalent runtime behavior. Inputs and content sidecars remain
read-only and outside version control.

## Measured performance, not universal superiority

The runtime benchmark now enables the authorization policy it measures and
creates fixtures outside the timed operation. With 1,000 stored objects,
median authorized dispatch changed from 497.5 microseconds to 544.7
nanoseconds, and a create/set/commit transaction from 570.9 to 50.65
microseconds (30 samples). These narrow operations benefit from copy-on-write
snapshots; mutating an entity still shallow-copies its record map. They do
not measure browser requests, SQLite publication, or complete model flows.

## Remaining high-priority behavior

- Executable complete-command oracles and implementation of missing commands.
- Whole-workspace exact line/branch coverage, then the additional function and
  region readiness checks; isolated component percentages are not global.
- Authenticated per-session runtime contexts, generated schemas and security
  policies, real model-flow execution and atomic runtime/SQLite lifecycle.
- Data-bound frontend loading, mutations, validation, action arguments,
  conditional behavior, all configured widgets and browser acceptance.
- Complete flow decompilation, domain/security/navigation semantics,
  OQL execution and transactional semantic refactoring.
- Official Studio Pro/MxBuild compatibility for each supported version and
  model family, plus controlled end-to-end performance regression budgets.

Known unsupported native page fields remain in the lossless snapshot. A page
becomes typed only when recompilation preserves its native fields and identity
references. That is loss prevention, not completion of its authoring support.

## Semantic refactoring

`rename`, `remove` and `move` mutate an `.mpr` directly rather than generating
source, so they follow MXRB's safety contract rather than the generators':
every command produces an inspectable plan, previews by default, and writes
only under `--apply`, inside a transaction. The `.mpr` is opened read-only
unless `--apply` was passed.

What is proven: a rename applied to a real `.mpr` rewrites both the artifact's
own declaration and the references to it, the old name is absent from every
string in the resulting file, and documentation prose is left untouched; a
removal is refused while any non-containment reference or child unit remains,
and the refusal exits nonzero without deleting anything; a move reports its
before/after containers and refuses a destination that would change the
artifact's qualified name.

Limits worth naming, because they are not obvious from the command names:

- A rename is a **boundary-guarded textual substitution** over decoded BSON
  strings, exactly as MXRB's is. It cannot distinguish a reference to
  `Sales.Order` from the same text in a document type nobody models. Free-text
  fields (`Documentation`, `Caption`, `Text`) are excluded so prose is not
  rewritten; every other field holding the name is. The per-string preview
  exists so this is auditable rather than trusted.
- `remove` and `move` operate on whole storage units. Modules, entities,
  attributes and associations are refused by name — they are parts of other
  documents, and changing them is a typed domain-model mutation. MXRB refuses
  the same set for the same reason.
- MXRB composes a cross-module move out of a move plus a rename. This refuses
  that case and names `rename` instead, rather than performing half of each.
- `update` is MXRB's **gem self-updater**, not model tooling; it queries a
  published release channel MXRS does not have. `design` is a theme/SCSS
  subsystem, unrelated to refactoring despite sharing this group in the
  backlog. Neither is implemented.

None of the three is verified: nothing compares their command contract against
MXRB's execution.

## Marketplace

`mxrs marketplace search|show|versions|download` speaks the official Mendix
Marketplace Content API (`https://marketplace-api.mendix.com/v1`). It is the
only command in the workspace that makes an outbound request.

What is proven: the client was run against the live API on 2026-09-14 and
downloaded Community Commons 11.5.1 as an 8,250,622-byte `.mpk` — a valid
archive of 128 entries with intact CRCs, containing `project.mpr` and
`package.xml`. Parsing, parameter validation, version selection, compatibility
checking and the redirect/credential policy are all covered offline against
responses recorded from that same API, so the suite needs neither a token nor
connectivity; one opt-in `#[ignore]`d test re-checks the live shape.

Credential handling is the part worth stating explicitly:

- The token is read from `MXRS_MENDIX_PAT` or the file named by
  `MXRS_MENDIX_PAT_FILE`, never from a command-line argument — an argument
  lands in shell history and in every process listing on the machine. There is
  no default path: reading a file the user did not name is how a credential
  gets used by surprise.
- `Pat` has a hand-written `Debug` printing `Pat(<redacted>)`, so the token
  cannot reach a panic message or a test failure through a derived `Debug`.
- Nothing persists the token. MXRB offers to save one into its own credentials
  file; this deliberately does not.
- Two host allow-lists, not one. The download endpoint answers HTTP 303 towards
  `files.appstore.mendix.com`, which serves a pre-signed URL. That host may be
  *fetched from* but is never *sent the account token*; redirects are resolved
  by the API layer, one hop at a time, re-checking the host each time, because
  both the initial `downloadUrl` and every `Location` are server-controlled.

### Installing a package

`mxrs marketplace install <package.mpk> <file.mpr>` imports a module package
into an existing project. Like the refactoring commands it previews by default
and writes only under `--apply`.

What is proven: the genuine Community Commons `.mpk` — downloaded from the
live Marketplace — installs into a project built by `cargo mxrs build`,
placing 128 model units and 124 declared files, after which the project still
reads back and lists the new module. The refusals and the rollback are covered
by tests that build their own packages, so they need no network.

Two writes happen at once and have to agree: model units go into the `.mpr`
inside an `mxrs-mpr` transaction, and declared assets go onto the filesystem
beside it with a backup of anything they replace. If the assets fail, the
transaction has already rolled the units back and the backups are restored; a
test asserts the restore by making the second asset's destination a directory
after the first has been written.

Refusals, all mirroring MXRB:

- A module already present in the target, or a unit ID that collides with one,
  is refused — installing over either would silently replace an unrelated
  document.
- A model-version difference is refused unless `--allow-model-upgrade` is
  passed, and even then only forward: nothing here migrates a model. The real
  Community Commons package targets Mendix 10.24.0 and hits exactly this when
  installed into an 11.12.1 project.
- A manifest declaring a different version from the model it ships is refused
  as an inconsistent package.
- Declared paths that are absolute, contain `..`, or point into `.git`,
  `.mxrs`, `.mxrb`, `mprcontents` or a nested `.mpr` are refused. A package is
  a file from the internet; its paths are untrusted input.

Not covered: MXRB additionally resolves a dependency graph before installing
and can uninstall, neither of which is here, so `module add` stays unported.
`marketplace` is not verified either: nothing compares its command contract
against MXRB's execution.

One behaviour worth recording because it is invisible until it bites: a
Mendix v2 `.mpr` keeps unit contents in a sibling `mprcontents/` directory,
nested two levels deep. Published packages embed a self-contained v1 project,
but a v2 package would be unopenable without that sidecar, so the extractor
carries it along — implicitly, as part of the project. It is still refused as
a *declared* asset path, because shipping a project's storage as an
installable file would let a package overwrite the target's own units.

## Marketplace

`mxrs marketplace search|show|versions|download` speaks the official Mendix
Marketplace Content API (`https://marketplace-api.mendix.com/v1`). It is the
only command in the workspace that makes an outbound request.

What is proven: the client was run against the live API on 2026-09-14 and
downloaded Community Commons 11.5.1 as an 8,250,622-byte `.mpk` — a valid
archive of 128 entries with intact CRCs, containing `project.mpr` and
`package.xml`. Parsing, parameter validation, version selection, compatibility
checking and the redirect/credential policy are all covered offline against
responses recorded from that same API, so the suite needs neither a token nor
connectivity; one opt-in `#[ignore]`d test re-checks the live shape.

Credential handling is the part worth stating explicitly:

- The token is read from `MXRS_MENDIX_PAT` or the file named by
  `MXRS_MENDIX_PAT_FILE`, never from a command-line argument — an argument
  lands in shell history and in every process listing on the machine. There is
  no default path: reading a file the user did not name is how a credential
  gets used by surprise.
- `Pat` has a hand-written `Debug` printing `Pat(<redacted>)`, so the token
  cannot reach a panic message or a test failure through a derived `Debug`.
- Nothing persists the token. MXRB offers to save one into its own credentials
  file; this deliberately does not.
- Two host allow-lists, not one. The download endpoint answers HTTP 303 towards
  `files.appstore.mendix.com`, which serves a pre-signed URL. That host may be
  *fetched from* but is never *sent the account token*; redirects are resolved
  by the API layer, one hop at a time, re-checking the host each time, because
  both the initial `downloadUrl` and every `Location` are server-controlled.

### Installing a package

`mxrs marketplace install <package.mpk> <file.mpr>` imports a module package
into an existing project. Like the refactoring commands it previews by default
and writes only under `--apply`.

What is proven: the genuine Community Commons `.mpk` — downloaded from the
live Marketplace — installs into a project built by `cargo mxrs build`,
placing 128 model units and 124 declared files, after which the project still
reads back and lists the new module. The refusals and the rollback are covered
by tests that build their own packages, so they need no network.

Two writes happen at once and have to agree: model units go into the `.mpr`
inside an `mxrs-mpr` transaction, and declared assets go onto the filesystem
beside it with a backup of anything they replace. If the assets fail, the
transaction has already rolled the units back and the backups are restored; a
test asserts the restore by making the second asset's destination a directory
after the first has been written.

Refusals, all mirroring MXRB:

- A module already present in the target, or a unit ID that collides with one,
  is refused — installing over either would silently replace an unrelated
  document.
- A model-version difference is refused unless `--allow-model-upgrade` is
  passed, and even then only forward: nothing here migrates a model. The real
  Community Commons package targets Mendix 10.24.0 and hits exactly this when
  installed into an 11.12.1 project.
- A manifest declaring a different version from the model it ships is refused
  as an inconsistent package.
- Declared paths that are absolute, contain `..`, or point into `.git`,
  `.mxrs`, `.mxrb`, `mprcontents` or a nested `.mpr` are refused. A package is
  a file from the internet; its paths are untrusted input.

Not covered: MXRB additionally resolves a dependency graph before installing
and can uninstall, neither of which is here, so `module add` stays unported.
`marketplace` is not verified either: nothing compares its command contract
against MXRB's execution.

One behaviour worth recording because it is invisible until it bites: a
Mendix v2 `.mpr` keeps unit contents in a sibling `mprcontents/` directory,
nested two levels deep. Published packages embed a self-contained v1 project,
but a v2 package would be unopenable without that sidecar, so the extractor
carries it along — implicitly, as part of the project. It is still refused as
a *declared* asset path, because shipping a project's storage as an
installable file would let a package overwrite the target's own units.
