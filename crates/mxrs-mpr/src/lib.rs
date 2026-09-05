//! Low-level Mendix `.mpr` SQLite/BSON unit I/O.
//!
//! Skeleton crate — not yet implemented. Scope (see
//! `lib/mxrb/io/mpr_file.rb` and `lib/mxrb/io/mxunit_codec.rb` in mxrb, and
//! milestones M1.2-M1.4 in the mxrs plan): open/create the `Unit`/`_MetaData`
//! SQLite tables, read/write the v2 (`.mxunit` content-addressed file)
//! storage format used by Studio Pro 10+, transactional inserts/updates/
//! deletes/relocates.
