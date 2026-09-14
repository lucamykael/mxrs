# Verification and remaining parity work

Passing tests, covering source lines, compiling a model, running that model,
and matching Studio Pro are different claims. None substitutes for the others.
The target is complete behavior and exact 100% coverage, not a rounded score.

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

The following readiness gates intentionally fail while work remains. They do
not accept source exclusions, missing instrumentation, truncated inventories,
rounded percentages, or a partial command implementation as complete:

```sh
cargo run -p xtask -- capability-matrix --check-baseline --require-complete
cargo run -p xtask -- coverage-gate target/coverage.json --require-complete
```

The command inventory currently covers MXRB's 76 top-level commands, not an
exhaustive list of Studio Pro capabilities. All 76 still need complete
command-contract evidence: 42 have partial equivalents and 34 are absent.

MXRB's model-authoring generators write Ruby source into a project; the MXRS
equivalents write Rust declarations into `src/domain/modules/`. Because the
generated content differs by construction, no differential oracle can compare
it, and those rows stay partial rather than verified however many tests pass.
What is proven is narrower and stated as such: a project scaffolded with
`module`/`entity`/`enumeration`/`use-case`/`nanoflow`/`page`/`published-rest`/
`consumed-rest`/`java-action`/`security` compiles with real Cargo, runs, and
the declarations reach the written `.mpr`; the argument grammar, rendering,
dry-run and registry behavior are covered by CLI tests. Nothing here
establishes that Studio Pro accepts the result.

`constant` and `scheduled-event` remain absent on purpose. Their MXRB
templates emit `constant`/`scheduled_event` declarations, and `mxrs-ir` has no
`ConstantDecl` or `ScheduledEventDecl` for them to compile against —
`mxrs-writer` persists neither `Constants$Constant` nor
`ScheduledEvents$ScheduledEvent`. A scaffold that emitted those calls anyway
would break `cargo check` for the user's whole project, so the commands are
not registered at all.

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
that the current run generated a valid model. Private-corpus artifacts are
never overwritten to run this check.

## Private-model acceptance

The six private-corpus tests are ignored by the portable suite because their
input projects cannot be committed. Invoking them explicitly without their
prerequisites fails; it is never a successful skip. Use an authorized local
corpus with this layout, including complete MPR content directories and assets:

```text
corpus/
  qrqc-ruby/build/eQRQC.mpr
  qrqc-ruby/build/mprcontents/
  spc-ruby/build/JEMScc-SPC.mpr
  spc-ruby/build/mprcontents/
  spc-ruby/build/deployment/model/model.mdp
```

```sh
export MXRS_ACCEPTANCE_DIR=/path/to/corpus
export MXRS_PAGE_BUNDLE_ORACLE_MPR="$MXRS_ACCEPTANCE_DIR/spc-ruby/build/JEMScc-SPC.mpr"
export MXRS_MODEL_PACKAGE_ORACLE="$MXRS_ACCEPTANCE_DIR/spc-ruby/build/deployment/model/model.mdp"
cargo test --no-fail-fast \
  -p mxrs-compiler-flow -p mxrs-compiler-widgets -p mxrs-pluggable -p mxrs-schema \
  --test acceptance_qrqc_spc --test loop_oracle --test page_bundle_oracle \
  --test instance_oracle --test schema_oracle --test model_package_oracle \
  -- --ignored --nocapture
```

Read-only acceptance on the locally audited corpus established:

- SPC: 787/787 flow compilations, 256/256 code actions, 169/169 nanoflows,
  167 page bundles without reported unsupported widgets, 682/682 widget
  schemas and instance semantic round trips.
- QRQC: 807/808 flow compilations, 225/225 code actions, 211/211 nanoflows,
  378/378 widget schemas and instance semantic round trips. The remaining
  failure references an association absent from the source domain model;
  the acceptance command still fails, and the reference is not invented or
  silently dropped.
- 118 SPC loop structures and connections matched the official package;
  another 15 editor loops belong to roots absent from that package.
- 1,761 ordered official model-package entries round-tripped byte-identically.

These checks do not prove that every generated widget works in a browser,
that an entire widget rewrites byte-identically, or that the application has
equivalent runtime behavior. Input MPR, MDP and content hashes must remain
unchanged. New corpus versions require their own evidence.

## Measured performance, not universal superiority

On the same local SPC corpus, compiling the 167 page bundles took
138.17 seconds before the widget-ID fast paths, 109.59 seconds with those
fast paths, and 25.95 seconds after also sharing the package-module cache
within each batch. The ordered generated page-source digest remained
`m5VV55xa05k3v2+1SubB0vdF8luUp2NVS3r2DArucgs=`. This is a debug-build,
single-machine observation (about 5.32x faster), not a release latency budget
or a comparison against Studio Pro. The cache is discarded between batches
so edits to widget packages are observed.

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
