//! Shared only by test harnesses. Coverage runs isolate nested Cargo builds
//! from both the developer cache and the outer Cargo process's artifact lock.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn target_dir(fallback: impl AsRef<Path>) -> PathBuf {
    select_target_dir(
        std::env::var_os("MXRS_TEST_CARGO_TARGET_DIR"),
        fallback.as_ref(),
    )
}

fn select_target_dir(override_path: Option<OsString>, fallback: &Path) -> PathBuf {
    override_path
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_builds_use_the_explicit_run_directory_or_preserve_the_normal_cache() {
        let fallback = Path::new("workspace/target");
        assert_eq!(select_target_dir(None, fallback), fallback);
        assert_eq!(
            select_target_dir(Some(OsString::from("coverage-run/nested")), fallback),
            Path::new("coverage-run/nested"),
        );
    }
}
