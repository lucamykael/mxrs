fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let mpr = arguments
        .next()
        .ok_or("usage: materialize <project.mpr> <web-directory>")?;
    let output = arguments
        .next()
        .ok_or("usage: materialize <project.mpr> <web-directory>")?;
    if arguments.next().is_some() {
        return Err("usage: materialize <project.mpr> <web-directory>".into());
    }
    let report = mxrs_materializers::materialize_mpr(mpr, output)?;
    println!(
        "materialized {} changed, {} unchanged, {} stale removed at {}",
        report.changed_files,
        report.unchanged_files,
        report.removed_stale_files,
        report.output.display()
    );
    Ok(())
}
