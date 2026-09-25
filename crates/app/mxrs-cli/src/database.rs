//! Docker-backed, project-isolated PostgreSQL workspaces.
//!
//! This is deliberately narrower than mxrb's database command: it manages
//! PostgreSQL only. Mendix Runtime boot, schema synchronization and workload
//! statistics remain outside this surface.

use std::path::{Path, PathBuf};
use std::process::Command;

use mxrs_oql::plan::{IndexCatalogEntry, PlanAnalyzer, PlanReport};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const FORMAT: u32 = 1;
const IMAGE: &str = "postgres:13-alpine";
const USER: &str = "mxrs";
const DATABASE: &str = "mxrs";
const MANAGED_LABEL: &str = "io.mxrs.managed";
const PROJECT_LABEL: &str = "io.mxrs.project";
/// How PostgreSQL and psql label their own diagnostics on stderr. Used to
/// tell a rejected statement apart from an unreachable container.
const PSQL_DIAGNOSTICS: [&str; 3] = ["ERROR:", "FATAL:", "psql:"];

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("invalid PostgreSQL port {0:?}")]
    InvalidPort(String),
    #[error("{0}: database workspace has not been initialized; run `mxrs db up`")]
    MissingState(String),
    #[error("database workspace port is {actual}, not requested port {requested}")]
    PortMismatch { actual: u16, requested: u16 },
    #[error("refusing to modify unowned Docker {kind} {name}")]
    Unowned { kind: &'static str, name: String },
    #[error("Docker is unavailable: {0}")]
    Docker(String),
    #[error("invalid database workspace state: {0}")]
    InvalidState(String),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid MPR: {0}")]
    InvalidMpr(String),
    /// A malformed online query or parameter set. Distinct from operational
    /// failures so `mxrs serve` can blame the request (HTTP 400), not the
    /// database (HTTP 422).
    #[error("{0}")]
    Query(String),
    #[error("PostgreSQL returned invalid CSV: {0}")]
    InvalidCsv(String),
    #[error("PostgreSQL returned invalid EXPLAIN JSON: {0}")]
    InvalidExplain(String),
    #[error("PostgreSQL returned an unreadable index catalog: {0}")]
    InvalidIndexCatalog(String),
    #[error(transparent)]
    Plan(#[from] mxrs_oql::plan::PlanError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseStatus {
    pub source: PathBuf,
    pub container: String,
    pub volume: String,
    pub image: &'static str,
    pub port: u16,
    pub initialized: bool,
    pub container_state: String,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseCredentials {
    pub host: &'static str,
    pub port: u16,
    pub database: &'static str,
    pub username: &'static str,
    pub password: String,
    pub url: String,
}

impl std::fmt::Debug for DatabaseCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DatabaseCredentials")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("url", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    format: u32,
    source: PathBuf,
    port: u16,
}

#[derive(Debug, Clone)]
pub struct DatabaseWorkspace {
    source: PathBuf,
    key: String,
    container: String,
    volume: String,
    state_path: PathBuf,
    secret_path: PathBuf,
    requested_port: u16,
}

impl DatabaseWorkspace {
    pub fn open(source: impl AsRef<Path>, port: u16) -> Result<Self, DatabaseError> {
        if port == 0 {
            return Err(DatabaseError::InvalidPort(port.to_string()));
        }
        let source =
            std::path::absolute(source.as_ref()).map_err(|error| io(source.as_ref(), error))?;
        let project = mxrs_model::Project::open(&source, true)
            .map_err(|error| DatabaseError::InvalidMpr(error.to_string()))?;
        drop(project);
        let state_root = std::env::var_os("MXRS_STATE_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(std::env::temp_dir)
            .join("mxrs/db");
        Ok(Self::at(source, port, state_root))
    }

    fn at(source: PathBuf, port: u16, state_root: PathBuf) -> Self {
        let key = format!("{:x}", Sha256::digest(source.to_string_lossy().as_bytes()));
        let short = &key[..12];
        Self {
            source,
            key: key.clone(),
            container: format!("mxrs-{short}-postgres"),
            volume: format!("mxrs-{short}-postgres-data"),
            state_path: state_root.join(format!("{key}.json")),
            secret_path: state_root.join(format!("{key}.password")),
            requested_port: port,
        }
    }

    pub fn status(&self) -> Result<DatabaseStatus, DatabaseError> {
        self.status_with(&CommandDocker)
    }

    pub fn up(&self) -> Result<DatabaseStatus, DatabaseError> {
        self.up_with(&CommandDocker)
    }

    pub fn down(&self) -> Result<DatabaseStatus, DatabaseError> {
        self.down_with(&CommandDocker)
    }

    pub fn destroy(&self) -> Result<DatabaseStatus, DatabaseError> {
        self.destroy_with(&CommandDocker)
    }

    pub fn credentials(&self) -> Result<DatabaseCredentials, DatabaseError> {
        let state = self
            .read_state()?
            .ok_or_else(|| DatabaseError::MissingState(self.state_path.display().to_string()))?;
        let password = std::fs::read_to_string(&self.secret_path)
            .map_err(|error| io(&self.secret_path, error))?
            .trim()
            .to_string();
        if password.is_empty() {
            return Err(DatabaseError::InvalidState(
                "empty database password".to_string(),
            ));
        }
        Ok(DatabaseCredentials {
            host: "127.0.0.1",
            port: state.port,
            database: DATABASE,
            username: USER,
            url: format!(
                "postgresql://{USER}:{password}@127.0.0.1:{}/{DATABASE}",
                state.port
            ),
            password,
        })
    }

    /// Runs one read-only `SELECT`/`WITH` statement and returns its rows as
    /// column-name maps. Ports mxrb's `DatabaseWorkspace#query_rows`: named
    /// `:parameter`s bind through psql variables (never string interpolation),
    /// results stream out as CSV, and a SQL `NULL` becomes `Value::Null`
    /// while an empty string stays `""`. mxrb enforces read-only access with
    /// a dedicated reader role; this workspace has a single owner role, so
    /// the same guarantee comes from `default_transaction_read_only` plus the
    /// single-statement validation.
    pub fn query_rows(
        &self,
        sql: &str,
        params: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>, DatabaseError> {
        self.query_rows_with(&CommandDocker, sql, params)
    }

    /// Runs one statement and returns psql's own rendering, the way
    /// `mxrb db sql` does — tables for a `SELECT`, a command tag for
    /// anything else. [`query_rows`](Self::query_rows) is the machine-facing
    /// path and stays CSV.
    ///
    /// mxrb reaches read-only by connecting as a dedicated reader role and
    /// wrapping the statement in `BEGIN READ ONLY` / `COMMIT`. This
    /// workspace has a single owner role (its containers are per-project and
    /// loopback-only), so the guarantee comes from
    /// `default_transaction_read_only`, which the server applies to every
    /// transaction in the session and which no statement in the payload can
    /// turn off. `--write` is the only way past it, and it is never implied.
    pub fn execute(&self, sql: &str, write: bool) -> Result<String, DatabaseError> {
        self.execute_with(&CommandDocker, sql, write)
    }

    fn execute_with(
        &self,
        docker: &impl Docker,
        sql: &str,
        write: bool,
    ) -> Result<String, DatabaseError> {
        if sql.trim().is_empty() {
            return Err(DatabaseError::Query("SQL must not be empty".to_string()));
        }
        if sql.contains('\0') {
            return Err(DatabaseError::Query("SQL contains a NUL byte".to_string()));
        }
        let mut arguments = self.psql_arguments(write);
        arguments.extend(["--command".to_string(), sql.to_string()]);
        classify(docker.output(&arguments)?)
    }

    /// Analyzes one read-only statement's PostgreSQL plan, matched against the
    /// indexes that already exist. Ports mxrb's `DatabaseWorkspace#explain`.
    ///
    /// `analyze` is opt-in because `EXPLAIN ANALYZE` executes the statement.
    /// mxrb keeps that safe with a dedicated reader role; here the session is
    /// created with `default_transaction_read_only`, which the server applies
    /// to every transaction in it. That matters for more than tidiness: a
    /// data-modifying CTE (`WITH written AS (DELETE ... RETURNING *) SELECT
    /// ...`) passes the read-only *statement* check, and it is the server
    /// setting — not the shape of the SQL — that refuses to run it.
    pub fn explain(&self, sql: &str, analyze: bool) -> Result<PlanReport, DatabaseError> {
        self.explain_with(&CommandDocker, sql, analyze)
    }

    fn explain_with(
        &self,
        docker: &impl Docker,
        sql: &str,
        analyze: bool,
    ) -> Result<PlanReport, DatabaseError> {
        let statement = read_only_statement(sql)?;
        let mut arguments = self.psql_arguments(false);
        arguments.extend([
            "--tuples-only".to_string(),
            "--no-align".to_string(),
            "--quiet".to_string(),
            "--command".to_string(),
            explain_sql(&statement, analyze),
        ]);
        let rendered = classify(docker.output(&arguments)?)?;
        let payload: Value = serde_json::from_str(rendered.trim())
            .map_err(|error| DatabaseError::InvalidExplain(error.to_string()))?;
        // The catalog is read only once the plan is in hand, so a rejected
        // statement costs one query rather than two.
        let indexes = self.index_catalog_with(docker)?;
        Ok(PlanAnalyzer::new(indexes).analyze(&payload, analyze)?)
    }

    fn index_catalog_with(
        &self,
        docker: &impl Docker,
    ) -> Result<Vec<IndexCatalogEntry>, DatabaseError> {
        self.query_rows_with(docker, INDEX_CATALOG_SQL, &Map::new())?
            .into_iter()
            .map(|row| {
                serde_json::from_value(Value::Object(row))
                    .map_err(|error| DatabaseError::InvalidIndexCatalog(error.to_string()))
            })
            .collect()
    }

    /// The `docker exec ... psql` prefix both statement paths share, up to but
    /// not including the command. Read-only unless `write`.
    fn psql_arguments(&self, write: bool) -> Vec<String> {
        let mut arguments = vec!["exec".to_string()];
        if !write {
            arguments.extend([
                "--env".to_string(),
                "PGOPTIONS=-c default_transaction_read_only=on".to_string(),
            ]);
        }
        arguments.extend([
            self.container.clone(),
            "psql".to_string(),
            "--no-psqlrc".to_string(),
            "--set".to_string(),
            "ON_ERROR_STOP=1".to_string(),
            "--username".to_string(),
            USER.to_string(),
            "--dbname".to_string(),
            DATABASE.to_string(),
        ]);
        arguments
    }

    /// The argv for an interactive `psql`, for the caller to exec.
    ///
    /// Returned rather than spawned so the command is inspectable: a shell
    /// that silently connects with write access would be indistinguishable
    /// from one that does not until something is already changed.
    pub fn shell_command(&self, write: bool) -> Vec<String> {
        let mut arguments = vec!["docker".to_string(), "exec".to_string()];
        if !write {
            arguments.extend([
                "--env".to_string(),
                "PGOPTIONS=-c default_transaction_read_only=on".to_string(),
            ]);
        }
        arguments.extend([
            "--interactive".to_string(),
            "--tty".to_string(),
            self.container.clone(),
            "psql".to_string(),
            "--no-psqlrc".to_string(),
            "--username".to_string(),
            USER.to_string(),
            "--dbname".to_string(),
            DATABASE.to_string(),
        ]);
        arguments
    }

    fn query_rows_with(
        &self,
        docker: &impl Docker,
        sql: &str,
        params: &Map<String, Value>,
    ) -> Result<Vec<Map<String, Value>>, DatabaseError> {
        let statement = read_only_statement(sql)?;
        let (statement, variables) = bind_parameters(&statement, params)?;
        let mut arguments = vec![
            "exec".to_string(),
            "--env".to_string(),
            "PGOPTIONS=-c default_transaction_read_only=on".to_string(),
            self.container.clone(),
            "psql".to_string(),
            "--no-psqlrc".to_string(),
            "--set".to_string(),
            "ON_ERROR_STOP=1".to_string(),
        ];
        arguments.extend(variables);
        arguments.extend([
            "--username".to_string(),
            USER.to_string(),
            "--dbname".to_string(),
            DATABASE.to_string(),
            "--csv".to_string(),
            "--quiet".to_string(),
            "--command".to_string(),
            format!("COPY ({statement}) TO STDOUT WITH CSV HEADER"),
        ]);
        let output = docker.run(&arguments)?;
        parse_csv_rows(&output)
    }

    fn status_with(&self, docker: &impl Docker) -> Result<DatabaseStatus, DatabaseError> {
        let state = self.read_state()?;
        let port = match &state {
            Some(state) => {
                self.ensure_requested_port(state)?;
                state.port
            }
            None => self.requested_port,
        };
        let inspection = docker.inspect("container", &self.container)?;
        if let Some(inspection) = &inspection {
            self.ensure_owned("container", &self.container, inspection)?;
        }
        Ok(DatabaseStatus {
            source: self.source.clone(),
            container: self.container.clone(),
            volume: self.volume.clone(),
            image: IMAGE,
            port,
            initialized: state.is_some(),
            container_state: inspection
                .map_or_else(|| "absent".to_string(), |inspection| inspection.state),
        })
    }

    fn up_with(&self, docker: &impl Docker) -> Result<DatabaseStatus, DatabaseError> {
        let state = match self.read_state()? {
            Some(state) => {
                self.ensure_requested_port(&state)?;
                state
            }
            None => self.create_state()?,
        };
        if let Some(inspection) = docker.inspect("container", &self.container)? {
            self.ensure_owned("container", &self.container, &inspection)?;
            if inspection.state != "running" {
                docker.run(&["start".into(), self.container.clone()])?;
            }
            return self.status_with(docker);
        }
        if let Some(inspection) = docker.inspect("volume", &self.volume)? {
            self.ensure_owned("volume", &self.volume, &inspection)?;
        } else {
            docker.run(&[
                "volume".into(),
                "create".into(),
                "--label".into(),
                format!("{MANAGED_LABEL}=true"),
                "--label".into(),
                format!("{PROJECT_LABEL}={}", self.key),
                self.volume.clone(),
            ])?;
        }
        docker.run(&[
            "run".into(),
            "--detach".into(),
            "--name".into(),
            self.container.clone(),
            "--label".into(),
            format!("{MANAGED_LABEL}=true"),
            "--label".into(),
            format!("{PROJECT_LABEL}={}", self.key),
            "--publish".into(),
            format!("127.0.0.1:{}:5432", state.port),
            "--env".into(),
            format!("POSTGRES_USER={USER}"),
            "--env".into(),
            format!("POSTGRES_DB={DATABASE}"),
            "--env".into(),
            "POSTGRES_PASSWORD_FILE=/run/secrets/mxrs-db-password".into(),
            "--mount".into(),
            format!(
                "type=bind,source={},target=/run/secrets/mxrs-db-password,readonly",
                self.secret_path.display()
            ),
            "--mount".into(),
            format!(
                "type=volume,source={},target=/var/lib/postgresql/data",
                self.volume
            ),
            IMAGE.into(),
        ])?;
        self.status_with(docker)
    }

    fn down_with(&self, docker: &impl Docker) -> Result<DatabaseStatus, DatabaseError> {
        if let Some(inspection) = docker.inspect("container", &self.container)? {
            self.ensure_owned("container", &self.container, &inspection)?;
            if inspection.state == "running" {
                docker.run(&["stop".into(), self.container.clone()])?;
            }
        }
        self.status_with(docker)
    }

    fn destroy_with(&self, docker: &impl Docker) -> Result<DatabaseStatus, DatabaseError> {
        if let Some(inspection) = docker.inspect("container", &self.container)? {
            self.ensure_owned("container", &self.container, &inspection)?;
            docker.run(&["rm".into(), "--force".into(), self.container.clone()])?;
        }
        if let Some(inspection) = docker.inspect("volume", &self.volume)? {
            self.ensure_owned("volume", &self.volume, &inspection)?;
            docker.run(&["volume".into(), "rm".into(), self.volume.clone()])?;
        }
        for path in [&self.state_path, &self.secret_path] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(path, error)),
            }
        }
        self.status_with(docker)
    }

    fn ensure_owned(
        &self,
        kind: &'static str,
        name: &str,
        inspection: &Inspection,
    ) -> Result<(), DatabaseError> {
        if inspection.managed == "true" && inspection.project == self.key {
            Ok(())
        } else {
            Err(DatabaseError::Unowned {
                kind,
                name: name.to_string(),
            })
        }
    }

    fn ensure_requested_port(&self, state: &State) -> Result<(), DatabaseError> {
        if state.port == self.requested_port {
            Ok(())
        } else {
            Err(DatabaseError::PortMismatch {
                actual: state.port,
                requested: self.requested_port,
            })
        }
    }

    fn read_state(&self) -> Result<Option<State>, DatabaseError> {
        if !self.state_path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.state_path).map_err(|error| io(&self.state_path, error))?;
        let state: State = serde_json::from_slice(&bytes)
            .map_err(|error| DatabaseError::InvalidState(error.to_string()))?;
        if state.format != FORMAT || state.source != self.source || state.port == 0 {
            return Err(DatabaseError::InvalidState(
                "format, source, or port does not match this project".to_string(),
            ));
        }
        Ok(Some(state))
    }

    fn create_state(&self) -> Result<State, DatabaseError> {
        let state = State {
            format: FORMAT,
            source: self.source.clone(),
            port: self.requested_port,
        };
        let mut bytes = serde_json::to_vec(&state)
            .map_err(|error| DatabaseError::InvalidState(error.to_string()))?;
        bytes.push(b'\n');
        let password = uuid::Uuid::new_v4().simple().to_string();
        write_private(&self.secret_path, format!("{password}\n").as_bytes())?;
        // State is the commit marker. If writing it fails, the private orphan
        // secret is harmless and a retry replaces it; the inverse ordering
        // could leave an initialized workspace with no usable credential.
        write_private(&self.state_path, &bytes)?;
        Ok(state)
    }
}

#[derive(Debug, Clone)]
struct Inspection {
    managed: String,
    project: String,
    state: String,
}

trait Docker {
    fn output(&self, arguments: &[String]) -> Result<DockerOutput, DatabaseError>;

    fn run(&self, arguments: &[String]) -> Result<String, DatabaseError> {
        let output = self.output(arguments)?;
        if output.success {
            Ok(output.stdout)
        } else {
            Err(DatabaseError::Docker(output.stderr.trim().to_string()))
        }
    }

    fn inspect(&self, kind: &str, name: &str) -> Result<Option<Inspection>, DatabaseError> {
        let format = if kind == "container" {
            format!(
                "{{{{index .Config.Labels \"{MANAGED_LABEL}\"}}}}|{{{{index .Config.Labels \"{PROJECT_LABEL}\"}}}}|{{{{.State.Status}}}}"
            )
        } else {
            format!(
                "{{{{index .Labels \"{MANAGED_LABEL}\"}}}}|{{{{index .Labels \"{PROJECT_LABEL}\"}}}}|present"
            )
        };
        let output = self.output(&[
            kind.to_string(),
            "inspect".into(),
            "--format".into(),
            format,
            name.to_string(),
        ])?;
        if !output.success {
            let stderr = output.stderr.to_ascii_lowercase();
            if stderr.contains("no such") || stderr.contains("not found") {
                return Ok(None);
            }
            return Err(DatabaseError::Docker(output.stderr.trim().to_string()));
        }
        let values = output.stdout.trim().split('|').collect::<Vec<_>>();
        if values.len() != 3 {
            return Err(DatabaseError::Docker(
                "unexpected Docker inspection response".to_string(),
            ));
        }
        Ok(Some(Inspection {
            managed: values[0].to_string(),
            project: values[1].to_string(),
            state: values[2].to_string(),
        }))
    }
}

struct CommandDocker;

impl Docker for CommandDocker {
    fn output(&self, arguments: &[String]) -> Result<DockerOutput, DatabaseError> {
        let output = Command::new("docker")
            .args(arguments)
            .output()
            .map_err(|error| DatabaseError::Docker(error.to_string()))?;
        Ok(DockerOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

struct DockerOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), DatabaseError> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| io(parent, error))?;
    }
    let mut staging = tempfile::Builder::new()
        .prefix(".mxrs-db-")
        .tempfile_in(parent)
        .map_err(|error| io(parent, error))?;
    let staging_path = staging.path().to_path_buf();
    {
        let file = staging.as_file_mut();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| io(&staging_path, error))?;
        }
        file.write_all(bytes)
            .map_err(|error| io(&staging_path, error))?;
        file.sync_all().map_err(|error| io(&staging_path, error))?;
    }
    staging
        .persist(path)
        .map_err(|error| io(path, error.error))?;
    Ok(())
}

fn io(path: &Path, source: std::io::Error) -> DatabaseError {
    DatabaseError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Blames the right component for a failed `docker exec ... psql`.
///
/// `docker exec` passes the container command's exit status through, so a
/// failure is usually PostgreSQL rejecting the statement — and reporting
/// "Docker is unavailable: cannot execute CREATE TABLE in a read-only
/// transaction" blames the wrong component for something the caller can
/// actually fix. psql's own diagnostics mean the query failed; anything else
/// keeps the Docker classification, because a daemon or container failure
/// really is operational.
fn classify(output: DockerOutput) -> Result<String, DatabaseError> {
    if output.success {
        return Ok(output.stdout);
    }
    let stderr = output.stderr.trim().to_string();
    if PSQL_DIAGNOSTICS
        .iter()
        .any(|prefix| stderr.contains(prefix))
    {
        return Err(DatabaseError::Query(stderr));
    }
    Err(DatabaseError::Docker(stderr))
}

/// Ports mxrb's `explain_sql`. `COSTS`/`VERBOSE`/`SETTINGS` are always on so
/// an estimated plan still names its relations and non-default planner
/// settings; `ANALYZE` additionally executes the statement, which is why
/// `BUFFERS` and `TIMING` only make sense alongside it.
fn explain_sql(statement: &str, analyze: bool) -> String {
    let mut options = vec!["FORMAT JSON", "COSTS TRUE", "VERBOSE TRUE", "SETTINGS TRUE"];
    if analyze {
        options.extend(["ANALYZE TRUE", "BUFFERS TRUE", "TIMING TRUE"]);
    }
    format!("EXPLAIN ({}) {statement}", options.join(", "))
}

/// mxrb's `index_catalog` query, verbatim: the user schemas only, ordered so
/// a finding lists the indexes of a relation in a stable order.
const INDEX_CATALOG_SQL: &str = "SELECT schemaname, tablename, indexname, indexdef \
     FROM pg_indexes \
     WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
     ORDER BY schemaname, tablename, indexname";

/// Ports mxrb's `read_only_statement`: one statement, no NUL bytes, no `;`,
/// and it must start with `SELECT` or `WITH`. Error messages match mxrb's so
/// the served contract stays recognizable across both tools.
fn read_only_statement(sql: &str) -> Result<String, DatabaseError> {
    if sql.contains('\0') {
        return Err(DatabaseError::Query("SQL contains a NUL byte".to_string()));
    }
    let statement = sql.trim();
    if statement.is_empty() {
        return Err(DatabaseError::Query("SQL must not be empty".to_string()));
    }
    let read_only_keyword = ["SELECT", "WITH"].iter().any(|keyword| {
        statement.len() >= keyword.len()
            && statement[..keyword.len()].eq_ignore_ascii_case(keyword)
            && statement[keyword.len()..]
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
    });
    if !read_only_keyword || statement.contains(';') {
        return Err(DatabaseError::Query(
            "online queries must be one read-only SELECT or WITH statement".to_string(),
        ));
    }
    Ok(statement.to_string())
}

/// The `:name` references a statement binds, first-seen order, unique. A `::`
/// type cast never introduces a parameter, exactly like mxrb's
/// `(?<!:):([A-Za-z][A-Za-z0-9_]*)` scan.
fn statement_parameters(statement: &str) -> Vec<String> {
    let bytes = statement.as_bytes();
    let mut names = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':'
            && (index == 0 || bytes[index - 1] != b':')
            && bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic)
        {
            let mut end = index + 1;
            while bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                end += 1;
            }
            let name = statement[index + 1..end].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
            index = end;
        } else {
            index += 1;
        }
    }
    names
}

fn bind_parameters(
    statement: &str,
    params: &Map<String, Value>,
) -> Result<(String, Vec<String>), DatabaseError> {
    if params.is_empty() {
        return Ok((statement.to_string(), Vec::new()));
    }
    validate_parameter_names(params, statement)?;
    let bound = replace_parameters(statement, params);
    Ok((bound, parameter_arguments(params)?))
}

fn validate_parameter_names(
    params: &Map<String, Value>,
    statement: &str,
) -> Result<(), DatabaseError> {
    let invalid: Vec<&str> = params
        .keys()
        .map(String::as_str)
        .filter(|name| !valid_parameter_name(name))
        .collect();
    parameter_error("invalid parameter names", &invalid)?;
    let expected = statement_parameters(statement);
    let missing: Vec<&str> = expected
        .iter()
        .map(String::as_str)
        .filter(|name| !params.contains_key(*name))
        .collect();
    parameter_error("missing query parameter", &missing)?;
    let unused: Vec<&str> = params
        .keys()
        .map(String::as_str)
        .filter(|name| !expected.iter().any(|expected| expected == name))
        .collect();
    parameter_error("unused query parameters", &unused)
}

fn parameter_error(label: &str, names: &[&str]) -> Result<(), DatabaseError> {
    if names.is_empty() {
        Ok(())
    } else {
        Err(DatabaseError::Query(format!(
            "{label}: {}",
            names.join(", ")
        )))
    }
}

fn valid_parameter_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

fn replace_parameters(statement: &str, params: &Map<String, Value>) -> String {
    let bytes = statement.as_bytes();
    let mut bound = String::with_capacity(statement.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':'
            && (index == 0 || bytes[index - 1] != b':')
            && bytes.get(index + 1).is_some_and(u8::is_ascii_alphabetic)
        {
            let mut end = index + 1;
            while bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                end += 1;
            }
            let name = &statement[index + 1..end];
            match params.get(name) {
                Some(Value::Null) => bound.push_str("NULL"),
                _ => bound.push_str(&format!(":'mxrs_{name}'")),
            }
            index = end;
        } else {
            // A `:` is always ASCII, so byte indexing stays on character
            // boundaries only while copying byte by byte through a helper.
            let character_end = statement[index..]
                .chars()
                .next()
                .map_or(index + 1, |character| index + character.len_utf8());
            bound.push_str(&statement[index..character_end]);
            index = character_end;
        }
    }
    bound
}

fn parameter_arguments(params: &Map<String, Value>) -> Result<Vec<String>, DatabaseError> {
    let mut arguments = Vec::new();
    for (name, value) in params {
        let rendered = match value {
            Value::Null => continue,
            Value::String(value) => value.clone(),
            Value::Number(value) => value.to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Array(_) | Value::Object(_) => {
                return Err(DatabaseError::Query(format!(
                    "unsupported parameter value for {name}"
                )));
            }
        };
        arguments.push("--set".to_string());
        arguments.push(format!("mxrs_{name}={rendered}"));
    }
    Ok(arguments)
}

/// Parses psql's CSV output. Distinguishing a quoted empty field (`""`, an
/// empty string) from an unquoted one (SQL `NULL`) matters, so this cannot be
/// a naive `split(',')`.
fn parse_csv_rows(output: &str) -> Result<Vec<Map<String, Value>>, DatabaseError> {
    let records = parse_csv(output)?;
    let Some((header, rows)) = records.split_first() else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .map(|row| {
            header
                .iter()
                .enumerate()
                .map(|(column, name)| {
                    let value = row.get(column).map_or(Value::Null, |field| {
                        if field.quoted || !field.text.is_empty() {
                            Value::String(field.text.clone())
                        } else {
                            Value::Null
                        }
                    });
                    (name.text.clone(), value)
                })
                .collect()
        })
        .collect())
}

struct CsvField {
    text: String,
    quoted: bool,
}

fn parse_csv(input: &str) -> Result<Vec<Vec<CsvField>>, DatabaseError> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if in_quotes {
            if character == '"' {
                if characters.peek() == Some(&'"') {
                    text.push('"');
                    characters.next();
                } else {
                    in_quotes = false;
                }
            } else {
                text.push(character);
            }
            continue;
        }
        match character {
            '"' if text.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            ',' => {
                record.push(CsvField {
                    text: std::mem::take(&mut text),
                    quoted: std::mem::take(&mut quoted),
                });
            }
            '\r' if characters.peek() == Some(&'\n') => {}
            '\n' => {
                record.push(CsvField {
                    text: std::mem::take(&mut text),
                    quoted: std::mem::take(&mut quoted),
                });
                records.push(std::mem::take(&mut record));
            }
            _ => text.push(character),
        }
    }
    if in_quotes {
        return Err(DatabaseError::InvalidCsv(
            "unterminated quoted field".to_string(),
        ));
    }
    if !text.is_empty() || quoted || !record.is_empty() {
        record.push(CsvField { text, quoted });
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    struct MockDocker {
        responses: RefCell<VecDeque<DockerOutput>>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl MockDocker {
        fn new(responses: Vec<DockerOutput>) -> Self {
            Self {
                responses: RefCell::new(responses.into()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Docker for MockDocker {
        fn output(&self, arguments: &[String]) -> Result<DockerOutput, DatabaseError> {
            self.calls.borrow_mut().push(arguments.to_vec());
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| DatabaseError::Docker("unexpected call".to_string()))
        }
    }

    fn output(success: bool, stdout: &str, stderr: &str) -> DockerOutput {
        DockerOutput {
            success,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    fn workspace(directory: &tempfile::TempDir) -> DatabaseWorkspace {
        DatabaseWorkspace::at(
            directory.path().join("app.mpr"),
            55_432,
            directory.path().join("state"),
        )
    }

    #[test]
    fn state_and_credentials_are_private_and_never_debug_the_password() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        workspace.create_state().unwrap();
        let credentials = workspace.credentials().unwrap();
        assert!(credentials.url.contains(&credentials.password));
        assert!(!format!("{credentials:?}").contains(&credentials.password));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&workspace.secret_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn up_labels_resources_and_keeps_the_password_out_of_docker_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let docker = MockDocker::new(vec![
            output(false, "", "No such container"),
            output(false, "", "no such volume"),
            output(true, "volume\n", ""),
            output(true, "container\n", ""),
            output(true, &format!("true|{}|running\n", workspace.key), ""),
        ]);
        let report = workspace.up_with(&docker).unwrap();
        assert_eq!(report.container_state, "running");
        let password = workspace.credentials().unwrap().password;
        let arguments = docker.calls.borrow().concat().join(" ");
        assert!(arguments.contains(MANAGED_LABEL));
        assert!(!arguments.contains(&password));
    }

    #[test]
    fn lifecycle_refuses_an_existing_unowned_container() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        workspace.create_state().unwrap();
        let docker = MockDocker::new(vec![output(true, "false|other|running\n", "")]);
        assert!(matches!(
            workspace.down_with(&docker),
            Err(DatabaseError::Unowned {
                kind: "container",
                ..
            })
        ));
        assert_eq!(docker.calls.borrow().len(), 1);
    }

    #[test]
    fn down_stops_a_running_owned_container_and_reports_the_new_state() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        workspace.create_state().unwrap();
        let docker = MockDocker::new(vec![
            output(true, &format!("true|{}|running\n", workspace.key), ""),
            output(true, "stopped\n", ""),
            output(true, &format!("true|{}|exited\n", workspace.key), ""),
        ]);
        let report = workspace.down_with(&docker).unwrap();
        assert_eq!(report.container_state, "exited");
        assert_eq!(docker.calls.borrow()[1], ["stop", &workspace.container]);
    }

    #[test]
    fn destroy_removes_owned_resources_and_private_state() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        workspace.create_state().unwrap();
        let docker = MockDocker::new(vec![
            output(true, &format!("true|{}|exited\n", workspace.key), ""),
            output(true, "removed\n", ""),
            output(true, &format!("true|{}|present\n", workspace.key), ""),
            output(true, "removed\n", ""),
            output(false, "", "no such container"),
        ]);
        let report = workspace.destroy_with(&docker).unwrap();
        assert_eq!(report.container_state, "absent");
        assert!(!report.initialized);
        assert!(!workspace.state_path.exists());
        assert!(!workspace.secret_path.exists());
        assert!(
            docker
                .calls
                .borrow()
                .iter()
                .any(|arguments| arguments.starts_with(&["volume".into(), "rm".into()]))
        );
    }

    #[test]
    fn docker_failures_are_not_reclassified_as_missing_resources() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let docker = MockDocker::new(vec![output(false, "", "daemon permission denied")]);
        assert!(matches!(
            workspace.status_with(&docker),
            Err(DatabaseError::Docker(message)) if message == "daemon permission denied"
        ));
        assert!(
            io(Path::new("state.json"), std::io::Error::other("denied"))
                .to_string()
                .contains("state.json")
        );
    }

    #[test]
    fn query_rows_binds_parameters_through_psql_variables_and_enforces_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let docker = MockDocker::new(vec![output(
            true,
            "name,total\nSales,\"\"\n,\"a\"\"b\"\n",
            "",
        )]);
        let mut params = Map::new();
        params.insert("minimum".to_string(), Value::from(5));
        params.insert("label".to_string(), Value::Null);
        let rows = workspace
            .query_rows_with(
                &docker,
                "  SELECT name, total FROM orders WHERE total > :minimum AND label IS :label ",
                &params,
            )
            .unwrap();
        let call = &docker.calls.borrow()[0];
        assert_eq!(
            call[..8],
            [
                "exec",
                "--env",
                "PGOPTIONS=-c default_transaction_read_only=on",
                workspace.container.as_str(),
                "psql",
                "--no-psqlrc",
                "--set",
                "ON_ERROR_STOP=1",
            ]
        );
        assert_eq!(call[8..10], ["--set", "mxrs_minimum=5"]);
        assert!(!call.iter().any(|argument| argument.contains("mxrs_label")));
        assert_eq!(
            call.last().unwrap(),
            "COPY (SELECT name, total FROM orders WHERE total > :'mxrs_minimum' AND label IS NULL) TO STDOUT WITH CSV HEADER"
        );
        assert_eq!(call[call.len() - 4..call.len() - 2], ["--csv", "--quiet"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], Value::String("Sales".to_string()));
        assert_eq!(rows[0]["total"], Value::String(String::new()));
        assert_eq!(rows[1]["name"], Value::Null);
        assert_eq!(rows[1]["total"], Value::String("a\"b".to_string()));
    }

    #[test]
    fn query_rows_rejects_everything_that_is_not_one_read_only_statement() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let empty = Map::new();
        for (sql, message) in [
            ("SELECT 1\0", "SQL contains a NUL byte"),
            ("   ", "SQL must not be empty"),
            (
                "DELETE FROM orders",
                "online queries must be one read-only SELECT or WITH statement",
            ),
            (
                "SELECTED review",
                "online queries must be one read-only SELECT or WITH statement",
            ),
            (
                "SELECT 1; DROP TABLE orders",
                "online queries must be one read-only SELECT or WITH statement",
            ),
        ] {
            let error = workspace
                .query_rows_with(&MockDocker::new(vec![]), sql, &empty)
                .unwrap_err();
            assert!(
                matches!(&error, DatabaseError::Query(actual) if actual == message),
                "{sql:?}: {error}"
            );
        }
        assert!(
            workspace
                .query_rows_with(
                    &MockDocker::new(vec![output(true, "", "")]),
                    "WITH t AS (SELECT 1) SELECT * FROM t",
                    &empty
                )
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn query_parameters_must_match_the_statement_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let case = |params: &[(&str, Value)], sql: &str| {
            let params = params
                .iter()
                .map(|(name, value)| ((*name).to_string(), value.clone()))
                .collect::<Map<String, Value>>();
            workspace
                .query_rows_with(&MockDocker::new(vec![]), sql, &params)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            case(&[("1bad", Value::from(1))], "SELECT :minimum"),
            "invalid parameter names: 1bad"
        );
        assert_eq!(
            case(&[("other", Value::from(1))], "SELECT :minimum, :other"),
            "missing query parameter: minimum"
        );
        assert_eq!(
            case(
                &[("minimum", Value::from(1)), ("extra", Value::from(2))],
                "SELECT :minimum"
            ),
            "unused query parameters: extra"
        );
        assert_eq!(
            case(&[("minimum", serde_json::json!([1]))], "SELECT :minimum"),
            "unsupported parameter value for minimum"
        );
        // A `::` cast is not a parameter reference.
        assert_eq!(
            case(&[("minimum", Value::from(1))], "SELECT '5'::int, :minimum"),
            case(&[("minimum", Value::from(1))], "SELECT :minimum"),
        );
    }

    #[test]
    fn a_persisted_port_cannot_be_silently_changed() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        workspace.create_state().unwrap();
        let changed = DatabaseWorkspace::at(
            workspace.source.clone(),
            55_433,
            directory.path().join("state"),
        );
        assert!(matches!(
            changed.status_with(&MockDocker::new(vec![])),
            Err(DatabaseError::PortMismatch { .. })
        ));
    }

    /// `db sql` is the human-facing path: psql's own rendering, one
    /// statement, read-only unless the caller says otherwise. The pin that
    /// matters is that read-only is the default and reaches the server as a
    /// session setting rather than as advice inside the payload.
    #[test]
    fn execute_runs_one_statement_read_only_unless_write_is_requested() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);

        let docker = MockDocker::new(vec![output(true, " count \n-------\n     3\n", "")]);
        let rendered = workspace
            .execute_with(&docker, "SELECT count(*) FROM orders", false)
            .unwrap();
        assert!(rendered.contains("3"), "{rendered}");
        assert_eq!(
            docker.calls.borrow()[0],
            [
                "exec",
                "--env",
                "PGOPTIONS=-c default_transaction_read_only=on",
                workspace.container.as_str(),
                "psql",
                "--no-psqlrc",
                "--set",
                "ON_ERROR_STOP=1",
                "--username",
                USER,
                "--dbname",
                DATABASE,
                "--command",
                "SELECT count(*) FROM orders",
            ]
        );

        // `--write` drops the read-only session setting and nothing else.
        let docker = MockDocker::new(vec![output(true, "UPDATE 1\n", "")]);
        workspace
            .execute_with(&docker, "UPDATE orders SET total = 1", true)
            .unwrap();
        let call = &docker.calls.borrow()[0];
        assert_eq!(call[0], "exec");
        assert_eq!(call[1], workspace.container.as_str());
        assert!(
            !call.iter().any(|argument| argument.contains("PGOPTIONS")),
            "{call:?}"
        );

        // The statement reaches psql verbatim: no quoting, splitting or
        // rewriting that could change what the server sees.
        let docker = MockDocker::new(vec![output(true, "", "")]);
        let awkward = "SELECT 'a;b', $$c'd$$ -- trailing";
        workspace.execute_with(&docker, awkward, false).unwrap();
        assert_eq!(docker.calls.borrow()[0].last().unwrap(), awkward);

        // A statement PostgreSQL rejects is a query failure, not an
        // operational one: "Docker is unavailable: cannot execute CREATE
        // TABLE in a read-only transaction" blames the wrong component for
        // something the caller can fix.
        let docker = MockDocker::new(vec![output(
            false,
            "",
            "ERROR:  cannot execute CREATE TABLE in a read-only transaction\n",
        )]);
        let error = workspace
            .execute_with(&docker, "CREATE TABLE t(id int)", false)
            .unwrap_err();
        assert!(
            matches!(&error, DatabaseError::Query(message)
                if message == "ERROR:  cannot execute CREATE TABLE in a read-only transaction"),
            "{error}"
        );
        // An unreachable container still reads as operational.
        let docker = MockDocker::new(vec![output(
            false,
            "",
            "Error response from daemon: No such container\n",
        )]);
        let error = workspace
            .execute_with(&docker, "SELECT 1", false)
            .unwrap_err();
        assert!(matches!(error, DatabaseError::Docker(_)), "{error}");

        // An empty or NUL-bearing statement is refused before Docker runs.
        for (sql, message) in [
            ("   ", "SQL must not be empty"),
            ("SELECT\0 1", "SQL contains a NUL byte"),
        ] {
            let docker = MockDocker::new(vec![]);
            let error = workspace.execute_with(&docker, sql, false).unwrap_err();
            assert_eq!(error.to_string(), message);
            assert!(docker.calls.borrow().is_empty(), "{sql:?} reached Docker");
        }
    }

    const PLAN: &str = r#"[{"Plan": {"Node Type": "Seq Scan", "Schema": "public",
        "Relation Name": "orders", "Plan Rows": 5000, "Total Cost": 1200,
        "Filter": "(status = 'Open')"}, "Planning Time": 0.5}]"#;
    const CATALOG: &str = "schemaname,tablename,indexname,indexdef\n\
        public,orders,orders_status_idx,CREATE INDEX orders_status_idx ON public.orders(status)\n\
        public,invoices,invoices_pkey,CREATE UNIQUE INDEX invoices_pkey ON public.invoices(id)\n";

    /// `db explain` asks PostgreSQL for machine-readable plan JSON and matches
    /// it against `pg_indexes`. The pins that matter: the plan query is
    /// read-only even under `--analyze` (which executes the statement), and
    /// only the scanned relation's indexes reach the finding.
    #[test]
    fn explain_analyzes_the_plan_against_the_index_catalog_and_stays_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        let docker = MockDocker::new(vec![output(true, PLAN, ""), output(true, CATALOG, "")]);

        let report = workspace
            .explain_with(&docker, "  SELECT * FROM orders  ", false)
            .unwrap();

        let call = &docker.calls.borrow()[0];
        assert_eq!(
            call.as_slice(),
            [
                "exec",
                "--env",
                "PGOPTIONS=-c default_transaction_read_only=on",
                workspace.container.as_str(),
                "psql",
                "--no-psqlrc",
                "--set",
                "ON_ERROR_STOP=1",
                "--username",
                USER,
                "--dbname",
                DATABASE,
                "--tuples-only",
                "--no-align",
                "--quiet",
                "--command",
                "EXPLAIN (FORMAT JSON, COSTS TRUE, VERBOSE TRUE, SETTINGS TRUE) SELECT * FROM orders",
            ]
        );
        assert!(
            docker.calls.borrow()[1]
                .last()
                .unwrap()
                .contains("pg_indexes"),
            "the index catalog is read after the plan"
        );
        assert!(!report.clean());
        let finding = &report.findings[0];
        assert_eq!(finding.relation.as_deref(), Some("public.orders"));
        assert_eq!(
            finding
                .indexes
                .iter()
                .map(|index| index.name.as_str())
                .collect::<Vec<_>>(),
            ["orders_status_idx"]
        );
        assert_eq!(
            report.planning_time_ms.as_ref().unwrap().as_f64(),
            Some(0.5)
        );

        // `--analyze` executes the statement, so the read-only session setting
        // is what keeps a data-modifying CTE from running. It must survive.
        let docker = MockDocker::new(vec![output(true, PLAN, ""), output(true, CATALOG, "")]);
        let report = workspace
            .explain_with(&docker, "SELECT * FROM orders", true)
            .unwrap();
        assert!(report.analyzed);
        let call = &docker.calls.borrow()[0];
        assert_eq!(
            call[1..3],
            ["--env", "PGOPTIONS=-c default_transaction_read_only=on"]
        );
        assert_eq!(
            call.last().unwrap(),
            "EXPLAIN (FORMAT JSON, COSTS TRUE, VERBOSE TRUE, SETTINGS TRUE, ANALYZE TRUE, \
             BUFFERS TRUE, TIMING TRUE) SELECT * FROM orders"
        );
    }

    #[test]
    fn explain_refuses_writable_statements_and_names_unusable_planner_output() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);

        // The statement check runs before Docker, exactly as for an online
        // query: `db explain` is not a way to run an UPDATE.
        let docker = MockDocker::new(vec![]);
        let error = workspace
            .explain_with(&docker, "UPDATE orders SET total = 1", false)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "online queries must be one read-only SELECT or WITH statement"
        );
        assert!(docker.calls.borrow().is_empty());

        // Output that is not EXPLAIN JSON is reported as such rather than
        // surfacing as a parser panic or an empty report.
        let docker = MockDocker::new(vec![output(true, "QUERY PLAN\nSeq Scan on orders\n", "")]);
        let error = workspace
            .explain_with(&docker, "SELECT 1", false)
            .unwrap_err();
        assert!(
            matches!(&error, DatabaseError::InvalidExplain(message) if message.contains("expected")),
            "{error}"
        );

        // Valid JSON that is not a plan envelope keeps mxrb's wording.
        let docker = MockDocker::new(vec![
            output(true, r#"[{"Planning Time": 1}]"#, ""),
            output(true, CATALOG, ""),
        ]);
        let error = workspace
            .explain_with(&docker, "SELECT 1", false)
            .unwrap_err();
        assert_eq!(error.to_string(), "PostgreSQL EXPLAIN JSON has no Plan");

        // A statement PostgreSQL rejects is a query failure, not a Docker one.
        let docker = MockDocker::new(vec![output(
            false,
            "",
            "ERROR:  relation \"missing\" does not exist\n",
        )]);
        let error = workspace
            .explain_with(&docker, "SELECT * FROM missing", false)
            .unwrap_err();
        assert!(matches!(&error, DatabaseError::Query(_)), "{error}");

        // An index catalog row that is not what pg_indexes promises is named,
        // not silently dropped from the findings.
        let docker = MockDocker::new(vec![
            output(true, PLAN, ""),
            output(true, "tablename,indexname\norders,orders_status_idx\n", ""),
        ]);
        let error = workspace
            .explain_with(&docker, "SELECT * FROM orders", false)
            .unwrap_err();
        assert!(
            matches!(&error, DatabaseError::InvalidIndexCatalog(_)),
            "{error}"
        );
    }

    #[test]
    fn the_shell_command_is_interactive_and_read_only_by_default() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory);
        assert_eq!(
            workspace.shell_command(false),
            [
                "docker",
                "exec",
                "--env",
                "PGOPTIONS=-c default_transaction_read_only=on",
                "--interactive",
                "--tty",
                workspace.container.as_str(),
                "psql",
                "--no-psqlrc",
                "--username",
                USER,
                "--dbname",
                DATABASE,
            ]
        );
        let writable = workspace.shell_command(true);
        assert!(
            !writable
                .iter()
                .any(|argument| argument.contains("PGOPTIONS"))
        );
        // Both forms stay interactive; only the read-only setting differs.
        for form in [workspace.shell_command(false), writable] {
            assert!(form.contains(&"--interactive".to_string()));
            assert!(form.contains(&"--tty".to_string()));
        }
    }
}
