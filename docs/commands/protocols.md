# Protocol audit contract

`mxrs protocols FILE.mpr [--json] [--no-progress]` ports the complete offline
`mxrb protocols` audit. It inspects imported Marketplace modules; it does not
implement, install or execute messaging/industrial protocols.

## Recognition and results

Recognition requires an exact, evidenced AppStore GUID in the built-in registry.
**The current MXRB and MXRS registries contain no evidenced recognition GUIDs.**
Consequently, all imported Marketplace modules currently appear in the sorted
`unknown_marketplace_modules` list, and `connectors` is empty. Module names and
public Marketplace component IDs cannot substitute for GUID evidence.

Human output reports that no known connectors were found, then lists unknown
modules and the `mxrs modules FILE` hint when appropriate. JSON contains
`connectors`, `unknown_marketplace_modules` and the MPR basename in `project`.
Unnamed imported modules contribute an empty string. Ordinary local modules
are excluded. Hidden/protected imported modules remain in the audit.

The connector representation retains the native fields and sorted public
entity/microflow names; hidden connectors expose empty member lists. Recognition
of an actual connector must gain an evidenced GUID and new native acceptance
fixtures before it can be claimed. The oracle explicitly fails if the native
registry gains GUIDs, requiring those new cases instead of silently keeping an
empty-only test.

Both storage formats, human/JSON output, help and `--no-progress` are supported.
Invalid inputs fail with stderr and nonzero status. MXRS additionally rejects
unknown/repeated flags and extra arguments. The only successful human-output
normalization is the executable name in the module-listing hint; JSON is compared
as the complete object. The project is never mutated or compiled.

## Repeat verification

```sh
cargo build -p mxrs-cli -p xtask
target/debug/xtask command-oracle protocols target/debug/mxrs
```

Disposable MXRB-generated v1/v2 fixtures cover no imports, protected and visible
imports, Unicode/absent names, empty/unknown GUIDs, and tempting name/component-ID
false positives. Both actual CLIs run; hashes and inventory stay unchanged.
Verified means parity of this fail-closed audit, not connector recognition or
protocol implementation that neither current CLI provides.
