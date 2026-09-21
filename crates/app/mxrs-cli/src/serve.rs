//! `mxrs serve` — the loopback-only read-only query server over a project's
//! database workspace.
//!
//! Ports `bin/mxrb`'s `when "serve"`: the HTTP contract lives in
//! [`mxrs_query_server`]; this module only adapts the Docker-backed
//! [`DatabaseWorkspace`](crate::database::DatabaseWorkspace) into the
//! server's executor seam and classifies its failures, so a malformed query
//! is the client's fault (HTTP 400) while a database failure is not (422).

use serde_json::{Map, Value};

use crate::database::{DatabaseError, DatabaseWorkspace};
use mxrs_query_server::{QueryError, QueryRows};

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
