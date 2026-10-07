# `mxrs validate`

```sh
mxrs validate <file.mpr> [--json]
```

Checks that an `.mpr` is well-formed storage, read-only. It prints
`[mxrs] OK` and exits 0, or each error on standard error and exits 1;
warnings go to standard error either way. `--json` prints
`{valid, errors, warnings}` instead.

The checks are mxrb's `Integrity::Validator`, reported in its order:

1. The `Unit` and `_MetaData` tables, and the `Unit` columns `UnitID`,
   `ContainerID`, `ContainmentName` and `ContentsHash`. A model missing one
   is reported for that alone.
2. The unit tree: exactly one root, no blank or repeated `UnitID`, no unit
   in a container that does not exist.
3. Each unit, in storage order: its content exists and matches its
   `ContentsHash`, parses, states a `$Type` and the `$ID` of its unit. A
   mismatched `$ID` mxrb recorded as kept from a legacy model
   (`_MxrbCompatibility`) is a warning rather than an error. No nested `$ID`
   repeats, every object with one stores it first, and every AutoNumber
   attribute defaults to 1 or more.
4. For v2 storage, every unit has its `.mxunit` file (an error otherwise),
   and every file is a unit's (a warning otherwise).

mxrs adds one check after mxrb's: a containment cycle among units that
otherwise satisfy the tree's rules. `xtask command-oracle validate` runs both
CLIs over a clean model and seven corrupted ones and compares every line.
This is storage integrity, not Studio Pro's consistency check.
