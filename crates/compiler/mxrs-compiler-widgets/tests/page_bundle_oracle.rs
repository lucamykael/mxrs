use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use mxrs_compiler_widgets::ProjectPageBundleCompiler;
use mxrs_model::Project;

#[test]
#[ignore = "requires an explicit MXRS_PAGE_BUNDLE_ORACLE_MPR path"]
fn audits_page_bundle_coverage_against_a_real_project() {
    let path = std::env::var_os("MXRS_PAGE_BUNDLE_ORACLE_MPR")
        .map(PathBuf::from)
        .expect("set MXRS_PAGE_BUNDLE_ORACLE_MPR to a private .mpr project with its mprcontents directory before explicitly running this ignored test");
    assert!(
        path.is_file(),
        "MXRS_PAGE_BUNDLE_ORACLE_MPR is not a readable project file: {}",
        path.display()
    );

    let started = Instant::now();
    let project = Project::open(&path, true).expect("open real project");
    let compiler = ProjectPageBundleCompiler::new(&project).expect("index real project");
    println!("page bundle oracle indexed in {:?}", started.elapsed());
    let compilation_started = Instant::now();
    let mut native = BTreeMap::<String, usize>::new();
    let mut custom = BTreeMap::<String, usize>::new();
    let mut native_pages = BTreeMap::<String, Vec<String>>::new();
    let mut custom_pages = BTreeMap::<String, Vec<String>>::new();
    let mut pages = 0usize;
    let mut unsupported_instances = 0usize;
    let mut source_fingerprints = Vec::new();
    for result in compiler.compile_pages() {
        let bundle = result.expect("every real page must emit a module");
        pages += 1;
        source_fingerprints.push((
            bundle.qualified_name.clone(),
            mxrs_bson::contents_hash(bundle.source.as_bytes()),
        ));
        unsupported_instances += bundle.unsupported_widget_instances.len()
            + bundle.unsupported_custom_widget_instances.len();
        if !bundle.unsupported_widget_instances.is_empty()
            || !bundle.unsupported_custom_widget_instances.is_empty()
        {
            println!(
                "unsupported instances in {}: native={:?}, custom={:?}",
                bundle.qualified_name,
                bundle.unsupported_widget_instances,
                bundle.unsupported_custom_widget_instances,
            );
        }
        for kind in bundle.unsupported_widgets {
            *native.entry(kind.clone()).or_default() += 1;
            native_pages
                .entry(kind)
                .or_default()
                .push(bundle.qualified_name.clone());
        }
        for kind in bundle.unsupported_custom_widgets {
            *custom.entry(kind.clone()).or_default() += 1;
            custom_pages
                .entry(kind)
                .or_default()
                .push(bundle.qualified_name.clone());
        }
    }
    println!("page bundle oracle: {pages} pages");
    println!(
        "page bundles compiled in {:?}",
        compilation_started.elapsed()
    );
    source_fingerprints.sort_unstable();
    println!(
        "page bundle source digest: {}",
        mxrs_bson::contents_hash(&serde_json::to_vec(&source_fingerprints).unwrap())
    );
    println!("unsupported native kinds: {native:#?}");
    println!("unsupported native pages: {native_pages:#?}");
    println!("unsupported custom widget ids: {custom:#?}");
    println!("unsupported custom widget pages: {custom_pages:#?}");
    assert!(pages > 0);
    assert_eq!(
        unsupported_instances, 0,
        "unsupported widget instances remain even if their kind inventory is empty"
    );
    assert!(
        native.is_empty(),
        "unsupported native widget kinds remain: {native_pages:#?}"
    );
    assert!(
        custom.is_empty(),
        "unsupported custom widget ids remain: {custom_pages:#?}"
    );
}
