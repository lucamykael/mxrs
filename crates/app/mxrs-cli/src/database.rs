//! Docker-backed, project-isolated PostgreSQL workspaces.
//!
//! This is deliberately narrower than mxrb's database command: it manages
//! PostgreSQL only. Mendix Runtime boot, schema synchronization, query-plan
//! analysis, and interactive SQL remain outside this surface.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FORMAT: u32 = 1;
const IMAGE: &str = "postgres:13-alpine";
const USER: &str = "mxrs";
const DATABASE: &str = "mxrs";
const MANAGED_LABEL: &str = "io.mxrs.managed";
const PROJECT_LABEL: &str = "io.mxrs.project";

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
}
