//! `mxrs serve` — the loopback-only read-only query server over a project's
//! database workspace.
//!
//! Ports `bin/mxrb`'s `when "serve"`: the HTTP contract lives in
//! [`mxrs_query_server`]; this module adapts the Docker-backed
//! [`DatabaseWorkspace`](crate::database::DatabaseWorkspace) into the
//! server's executor seam and classifies its failures, so a malformed query
//! is the client's fault (HTTP 400) while a database failure is not (422).
//!
//! It also decides which table naming OQL translates to. Two legitimate
//! layouts exist: the Mendix Runtime's (`"Sales$Order"` — the only one mxrb's
//! server ever reads) and the physical one MXRS's own `db sync` writes
//! (`mxrb_entity_<hash>`). Serving OQL against the wrong one fails every
//! query with `relation ... does not exist`, so the choice is resolved once,
//! before the server starts, and announced — by probing for the catalog
//! `db sync` leaves behind, unless `--oql-layout` names a layout outright.

use std::str::FromStr;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::database::{DatabaseError, DatabaseWorkspace};
use mxrs_query_server::{LogicalOql, QueryError, QueryRows, TranslateOql};

pub struct WorkspaceQueries(pub DatabaseWorkspace);

impl QueryRows for WorkspaceQueries {
    fn query_rows(
        &self,
        sql: &str,
        params: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>, QueryError> {
        self.0.query_rows(sql, params).map_err(|error| match error {
            DatabaseError::Query(message) => QueryError::InvalidRequest(message),
            other => QueryError::Failed(other.to_string()),
        })
    }
}

/// Projects OQL onto the physical tables `db sync` writes, through the
/// catalog derived once from the model at startup.
pub struct PhysicalOql(pub mxrs_oql::RuntimeCatalog);

impl TranslateOql for PhysicalOql {
    fn translate(&self, oql: &str) -> mxrs_oql::Projection {
        mxrs_oql::translate_physical(oql, mxrs_oql::Dialect::PostgreSql, &self.0)
    }
}

/// Which table naming `serve` translates OQL to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OqlLayout {
    /// Ask the database: MXRS's own schema catalog means `db sync` wrote it.
    #[default]
    Auto,
    /// The typed tables `db sync` writes (`mxrb_entity_<hash>`).
    Physical,
    /// Mendix Runtime naming (`"Sales$Order"`) — mxrb's only behavior.
    Mendix,
}

impl FromStr for OqlLayout {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "physical" => Ok(Self::Physical),
            "mendix" => Ok(Self::Mendix),
            other => Err(format!(
                "unknown OQL layout {other:?}; use auto, physical or mendix"
            )),
        }
    }
}

/// Resolves the layout to a translator and a line saying what was chosen and
/// why, for the startup banner. `Auto` reads the database, so it needs the
/// workspace running; when it cannot answer, the error says to either start
/// the database or name a layout instead of guessing one.
///
/// # Errors
///
/// A human-readable message: the probe failed, or the model needed for the
/// physical catalog cannot be read.
pub fn oql_translator(
    layout: OqlLayout,
    workspace: &DatabaseWorkspace,
    source: &str,
) -> Result<(Arc<dyn TranslateOql>, String), String> {
    match layout {
        OqlLayout::Mendix => Ok((
            Arc::new(LogicalOql),
            "Mendix Runtime naming (--oql-layout mendix)".to_string(),
        )),
        OqlLayout::Physical => Ok((
            physical_translator(source)?,
            "physical tables (--oql-layout physical)".to_string(),
        )),
        OqlLayout::Auto => match workspace.catalog_present() {
            Ok(true) => Ok((
                physical_translator(source)?,
                "physical tables (the database carries MXRS's schema catalog)".to_string(),
            )),
            Ok(false) => Ok((
                Arc::new(LogicalOql),
                "Mendix Runtime naming (no MXRS schema catalog in the database)".to_string(),
            )),
            Err(error) => Err(format!(
                "cannot probe the database for its OQL layout: {error}; \
                 start the database (drop --no-up) or pass --oql-layout physical|mendix"
            )),
        },
    }
}

fn physical_translator(source: &str) -> Result<Arc<dyn TranslateOql>, String> {
    let project = mxrs_model::Project::open(source, true).map_err(|error| error.to_string())?;
    let catalog = mxrs_oql::RuntimeCatalog::from_project(&project)
        .map_err(|error| format!("cannot derive the physical catalog: {error}"))?;
    Ok(Arc::new(PhysicalOql(catalog)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_flag_names_its_three_values_and_rejects_the_rest() {
        assert_eq!("auto".parse::<OqlLayout>().unwrap(), OqlLayout::Auto);
        assert_eq!(
            "physical".parse::<OqlLayout>().unwrap(),
            OqlLayout::Physical
        );
        assert_eq!("mendix".parse::<OqlLayout>().unwrap(), OqlLayout::Mendix);
        assert_eq!(OqlLayout::default(), OqlLayout::Auto);
        let error = "runtime".parse::<OqlLayout>().unwrap_err();
        assert!(error.contains("auto, physical or mendix"), "{error}");
    }

    /// The same query the logical translator happily projects onto Mendix
    /// Runtime naming is refused by a physical translator whose catalog does
    /// not know the entity — the proof that [`PhysicalOql`] consults the
    /// catalog instead of guessing table names.
    #[test]
    fn a_physical_translator_answers_from_its_catalog_not_from_naming_rules() {
        let oql = "FROM Sales.Order AS o SELECT o/Total AS total";
        let logical = LogicalOql.translate(oql);
        assert!(logical.sql.unwrap().contains("\"sales$order\""));
        let physical = PhysicalOql(mxrs_oql::RuntimeCatalog::default()).translate(oql);
        assert!(physical.sql.is_none());
        assert!(physical.warnings[0].contains("Sales.Order"), "{physical:?}");
    }
}
