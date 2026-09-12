use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use mxrs_compiler_widgets::ProjectPageBundleCompiler;
use mxrs_model::Project;

#[test]
#[ignore = "requires MXRS_PAGE_BUNDLE_ORACLE_MPR or a local SPC checkout"]
fn audits_page_bundle_coverage_against_a_real_project() {
    let path = std::env::var_os("MXRS_PAGE_BUNDLE_ORACLE_MPR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "/home/mykael/Personal_Projects/spc-zero-errors-ruby/build/JEMScc-SPC.mpr",
            )
        });
    if !path.is_file() {
        eprintln!(
            "page bundle oracle skipped: {} is unavailable",
            path.display()
        );
        return;
    }

    let started = Instant::now();
    let project = Project::open(&path, true).expect("open real project");
    let compiler = ProjectPageBundleCompiler::new(&project).expect("index real project");
    println!("page bundle oracle indexed in {:?}", started.elapsed());
    let compilation_started = Instant::now();
    let mut native = BTreeMap::<String, usize>::new();
    let mut custom = BTreeMap::<String, usize>::new();
    let mut pages = 0usize;
    for result in compiler.compile_pages() {
        let bundle = result.expect("every real page must emit a module");
        pages += 1;
        for kind in bundle.unsupported_widgets {
            *native.entry(kind).or_default() += 1;
        }
        for kind in bundle.unsupported_custom_widgets {
            *custom.entry(kind).or_default() += 1;
        }
    }
    println!("page bundle oracle: {pages} pages");
    println!(
        "page bundles compiled in {:?}",
        compilation_started.elapsed()
    );
    println!("unsupported native kinds: {native:#?}");
    println!("unsupported custom widget ids: {custom:#?}");
    assert!(pages > 0);
}
