//! The browser ER-diagram editor and its background lifecycle — ports
//! `Mxrb::DomainDiagram::Server` and `Mxrb::DomainDiagram::Lifecycle`.
//!
//! The server binds only to loopback and works on a copy of the model (the
//! layout MPR), never the source. It serves the editor (mxrb's, see
//! `assets/diagram_er/NOTICE.md`), the diagram as JSON, and takes layout
//! changes signed with the token the page was given. A managed server runs
//! detached: `up` starts it, and `status`, `down` and `destroy` reach it
//! through a token-authenticated loopback endpoint rather than a PID that
//! may be stale.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAX_BODY_BYTES: usize = 2_097_152;
/// How much of a request line and headers a server reads.
const MAX_HEAD_BYTES: u64 = 16 * 1024;
/// How many connections a server serves at once.
const MAX_CONNECTIONS: usize = 64;
/// How long a request may take to arrive.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// The environment variable a managed worker reads its lifecycle token
/// from, so no other user sees it on the command line.
pub const LIFECYCLE_TOKEN_VARIABLE: &str = "MXRS_DIAGRAM_LIFECYCLE_TOKEN";
const LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "::1", "localhost"];
/// The port the editor is served on unless another is asked for.
pub const DEFAULT_PORT: u16 = 4568;
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(8);
/// The header the editor signs a layout with: mxrb's protocol.
const EDITOR_TOKEN: &str = "x-mxrb-token";
/// The header the lifecycle reaches a managed server with.
const LIFECYCLE_TOKEN: &str = "x-mxrs-lifecycle-token";

const PAGE: &[u8] = include_bytes!("../assets/diagram_er/domain.html");
const ASSETS: &[(&str, &[u8], &str)] = &[
    (
        "domain-qsjXhy4v.js",
        include_bytes!("../assets/diagram_er/assets/domain-qsjXhy4v.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "jsx-runtime-DV0a5kSb.js",
        include_bytes!("../assets/diagram_er/assets/jsx-runtime-DV0a5kSb.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "domain-mGOZ1KCG.css",
        include_bytes!("../assets/diagram_er/assets/domain-mGOZ1KCG.css"),
        "text/css; charset=utf-8",
    ),
];

/// A loopback server of the editor over a layout copy of a model.
pub struct Server {
    source: PathBuf,
    output: PathBuf,
    modules: Vec<String>,
    host: String,
    port: u16,
    token: String,
    lifecycle_token: Option<String>,
    managed_paths: Vec<PathBuf>,
    stopped: Arc<AtomicBool>,
    /// The port bound, once serving: the one asked for, or the one the
    /// system gave for port 0.
    bound: Arc<AtomicU16>,
}

/// How a server comes by its layout copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// A fresh copy; an existing one is replaced only when forced.
    Fresh { force: bool },
    /// The copy a managed server made before.
    Reuse,
}

impl Server {
    pub fn new(
        source: impl AsRef<Path>,
        output: Option<&Path>,
        modules: Vec<String>,
        port: u16,
        mode: Output,
        lifecycle_token: Option<String>,
    ) -> Result<Self, String> {
        let source = absolute(source.as_ref())?;
        if !source.is_file() {
            return Err(format!("MPR not found: {}", source.display()));
        }
        let output = match output {
            Some(output) => absolute(output)?,
            None => default_output(&source),
        };
        if same_file(&output, &source) {
            return Err("output must be a safe copy, not the source MPR".to_string());
        }
        // A v2 model keeps its units in `mprcontents/` beside it: a copy in
        // the same folder would share them, and a layout written into the
        // copy would be written into the model.
        let source_folder = source.parent().unwrap_or_else(|| Path::new("."));
        let output_folder = output.parent().unwrap_or_else(|| Path::new("."));
        if source_folder.join("mprcontents").is_dir() && output_folder == source_folder {
            return Err(format!(
                "output must be in another folder than the model: {} keeps its units in {}, which a copy beside it would share",
                source.display(),
                source_folder.join("mprcontents").display()
            ));
        }
        if output.is_dir() {
            return Err(format!(
                "output cannot be a directory: {}",
                output.display()
            ));
        }
        if output.is_symlink() {
            return Err(format!(
                "output cannot be a symbolic link: {}",
                output.display()
            ));
        }
        let mut server = Self {
            source,
            output,
            modules,
            host: "127.0.0.1".to_string(),
            port,
            token: random_token(),
            lifecycle_token,
            managed_paths: Vec::new(),
            stopped: Arc::new(AtomicBool::new(false)),
            bound: Arc::new(AtomicU16::new(port)),
        };
        server.managed_paths.push(server.output.clone());
        server.prepare_output(mode)?;
        Ok(server)
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    pub fn managed_paths(&self) -> &[PathBuf] {
        &self.managed_paths
    }

    /// A handle that stops [`Server::serve`] from another thread.
    pub fn stopper(&self) -> impl Fn() + Send + Sync + 'static {
        let stopped = self.stopped.clone();
        let bound = self.bound.clone();
        let host = self.host.clone();
        move || {
            stopped.store(true, Ordering::SeqCst);
            // Wakes the accept loop.
            let _ = TcpStream::connect((host.as_str(), bound.load(Ordering::SeqCst)));
        }
    }

    /// Serves until stopped; `ready` runs once the port is bound, with the
    /// port it is.
    pub fn serve(mut self, ready: impl FnOnce(u16)) -> Result<(), String> {
        if !LOOPBACK_HOSTS.contains(&self.host.as_str()) {
            return Err("domain model server must bind to loopback".to_string());
        }
        let listener = TcpListener::bind((self.host.as_str(), self.port))
            .map_err(|error| format!("cannot bind {}:{}: {error}", self.host, self.port))?;
        self.port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        self.bound.store(self.port, Ordering::SeqCst);
        ready(self.port);
        let server = Arc::new(self);
        let active = Arc::new(AtomicUsize::new(0));
        let mut handlers: Vec<std::thread::JoinHandle<()>> = Vec::new();
        for stream in listener.incoming() {
            if server.stopped.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = stream else { continue };
            handlers.retain(|handler| !handler.is_finished());
            if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                let _ = respond(
                    &mut stream,
                    503,
                    "application/json; charset=utf-8",
                    br#"{"ok":false,"error":"busy"}"#,
                );
                continue;
            }
            active.fetch_add(1, Ordering::SeqCst);
            let server = server.clone();
            let active = active.clone();
            handlers.push(std::thread::spawn(move || {
                server.handle(stream);
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        // What was asked before the server stopped is answered.
        for handler in handlers {
            let _ = handler.join();
        }
        Ok(())
    }

    fn prepare_output(&mut self, mode: Output) -> Result<(), String> {
        match mode {
            Output::Reuse => {
                if !self.output.is_file() {
                    return Err(format!("output not found: {}", self.output.display()));
                }
                Ok(())
            }
            Output::Fresh { force } => {
                if self.output.exists() && !force {
                    return Err(format!(
                        "output already exists: {}; use --force",
                        self.output.display()
                    ));
                }
                // The folder mxrs makes for a copy is its own: forced, the
                // contents an earlier copy left there are replaced. Any
                // other folder's are a project's, and are never touched.
                let own = self.output == default_output(&self.source);
                if force && own {
                    let stale = self
                        .output
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join("mprcontents");
                    if stale.is_dir() && !stale.is_symlink() {
                        std::fs::remove_dir_all(&stale).map_err(|error| {
                            format!("cannot replace {}: {error}", stale.display())
                        })?;
                    }
                }
                let contents = self.external_contents()?;
                let parent = self.output.parent().unwrap_or_else(|| Path::new("."));
                if !parent.exists() {
                    // A folder made for the copy goes with it.
                    self.managed_paths.insert(0, parent.to_path_buf());
                }
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
                std::fs::copy(&self.source, &self.output)
                    .map_err(|error| format!("cannot copy the model: {error}"))?;
                if let Some((from, to)) = contents {
                    copy_tree(&from, &to)?;
                    self.managed_paths.push(to);
                }
                Ok(())
            }
        }
    }

    /// The model's v2 contents folder and where its copy goes, beside the
    /// copy — always another folder than the model's; a copy there must not
    /// already exist.
    fn external_contents(&self) -> Result<Option<(PathBuf, PathBuf)>, String> {
        let source_folder = self.source.parent().unwrap_or_else(|| Path::new("."));
        let contents = source_folder.join("mprcontents");
        if !contents.is_dir() {
            return Ok(None);
        }
        let output_folder = self.output.parent().unwrap_or_else(|| Path::new("."));
        let target = output_folder.join("mprcontents");
        if target.exists() || target.is_symlink() {
            return Err(format!(
                "external contents destination already exists: {}",
                target.display()
            ));
        }
        Ok(Some((contents, target)))
    }

    fn handle(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(REQUEST_DEADLINE));
        let Some(request) = read_request(&stream) else {
            return;
        };
        // A page of another origin names its own host: one the browser
        // reached through a rebound name is refused.
        let port = self.bound.load(Ordering::SeqCst);
        let host_allowed = request.header("host").is_some_and(|host| {
            ["127.0.0.1", "localhost", "[::1]"]
                .iter()
                .any(|name| host == format!("{name}:{port}"))
        });
        if !host_allowed {
            let _ = respond(
                &mut stream,
                403,
                "application/json; charset=utf-8",
                br#"{"ok":false,"error":"forbidden host"}"#,
            );
            return;
        }
        let (status, kind, body) = self.dispatch(&request);
        let _ = respond(&mut stream, status, kind, &body);
        // A shutdown is answered before the server stops.
        if status == 200 && request.path == "/api/admin/shutdown" {
            (self.stopper())();
        }
    }

    fn dispatch(&self, request: &Request) -> (u16, &'static str, Vec<u8>) {
        let json_answer = |status: u16, value: Value| {
            (
                status,
                "application/json; charset=utf-8",
                value.to_string().into_bytes(),
            )
        };
        let not_found = || json_answer(404, json!({ "ok": false, "error": "not_found" }));
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => (200, "text/html; charset=utf-8", PAGE.to_vec()),
            ("GET", path) if path.starts_with("/assets/") => {
                let name = &path["/assets/".len()..];
                match ASSETS.iter().find(|(asset, _, _)| *asset == name) {
                    Some((_, bytes, kind)) => (200, kind, bytes.to_vec()),
                    None => not_found(),
                }
            }
            ("GET", "/api/diagram") => {
                match crate::diagram_er::document(&self.output, &self.modules) {
                    Ok(mut payload) => {
                        payload["output"] = json!(self.output.display().to_string());
                        payload["csrf_token"] = json!(self.token);
                        json_answer(200, payload)
                    }
                    Err(error) => json_answer(422, json!({ "ok": false, "error": error })),
                }
            }
            (method, path) if path.starts_with("/api/admin/") => {
                let authorized = self
                    .lifecycle_token
                    .as_deref()
                    .is_some_and(|token| request.header(LIFECYCLE_TOKEN) == Some(token));
                if !authorized {
                    return json_answer(403, json!({ "ok": false, "error": "forbidden" }));
                }
                match (method, path) {
                    ("GET", "/api/admin/status") => json_answer(200, json!({ "ok": true })),
                    ("POST", "/api/admin/shutdown") => json_answer(200, json!({ "ok": true })),
                    _ => not_found(),
                }
            }
            ("POST", "/api/layout") => match self.layout(request) {
                Ok(changed) => json_answer(
                    200,
                    json!({ "ok": true, "changed": changed, "output": self.output.display().to_string() }),
                ),
                Err(error) => json_answer(422, json!({ "ok": false, "error": error })),
            },
            _ => not_found(),
        }
    }

    fn layout(&self, request: &Request) -> Result<usize, String> {
        if request.header(EDITOR_TOKEN) != Some(self.token.as_str()) {
            return Err("invalid editor token".to_string());
        }
        if request.body.len() > MAX_BODY_BYTES {
            return Err("layout exceeds 2 MiB".to_string());
        }
        let payload: Value =
            serde_json::from_slice(&request.body).map_err(|error| error.to_string())?;
        let plan = crate::diagram_er::plan_layout(&self.output, &payload)?;
        crate::diagram_er::apply_layout(&self.output, plan)
    }
}

/// An HTTP request as the server reads one.
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn respond(stream: &mut TcpStream, status: u16, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// A request, read within [`MAX_HEAD_BYTES`] of request line and headers
/// and [`REQUEST_DEADLINE`] in all.
fn read_request(stream: &TcpStream) -> Option<Request> {
    let started = Instant::now();
    let mut head = BufReader::new(stream).take(MAX_HEAD_BYTES);
    let mut line = String::new();
    head.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    let path = target.split('?').next().unwrap_or(target).to_string();
    let mut headers = Vec::new();
    loop {
        if started.elapsed() > REQUEST_DEADLINE {
            return None;
        }
        let mut header = String::new();
        if head.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((key, value)) = header.split_once(':') {
            headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    // One byte past the limit is enough to refuse it.
    let mut body = Vec::new();
    head.into_inner()
        .take(length.min(MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .ok()?;
    if started.elapsed() > REQUEST_DEADLINE {
        return None;
    }
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        403 => "Forbidden",
        503 => "Service Unavailable",
        404 => "Not Found",
        _ => "Unprocessable Entity",
    }
}

/// `<stem>.domain-layout.mpr` beside the model, as mxrb's; for a v2 model,
/// whose units are in the folder's `mprcontents/`, in a folder of its own
/// (`<stem>.domain-layout/`) so the copy has contents of its own.
fn default_output(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = format!("{stem}.domain-layout.mpr");
    let folder = source.parent().unwrap_or_else(|| Path::new("."));
    if folder.join("mprcontents").is_dir() {
        folder.join(format!("{stem}.domain-layout")).join(name)
    } else {
        source.with_file_name(name)
    }
}

/// Whether two paths are one file: the same path, or the same file by its
/// device and inode — a hard link included.
fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    std::fs::canonicalize(left)
        .ok()
        .zip(std::fs::canonicalize(right).ok())
        .is_some_and(|(left, right)| left == right)
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)
                .map_err(|error| format!("{}: {error}", target.display()))?;
        }
    }
    Ok(())
}

fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

/// Where a managed editor is, as `status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub state: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

/// The background lifecycle of the editor of one model.
pub struct Lifecycle {
    source: PathBuf,
    state_root: PathBuf,
    executable: PathBuf,
}

impl Lifecycle {
    pub fn new(
        source: impl AsRef<Path>,
        state_root: PathBuf,
        executable: PathBuf,
    ) -> Result<Self, String> {
        Ok(Self {
            source: absolute(source.as_ref())?,
            state_root: absolute(&state_root)?,
            executable,
        })
    }

    /// `MXRS_DIAGRAM_STATE_ROOT`, else `~/.local/state/mxrs/diagram-er`.
    pub fn default_state_root() -> PathBuf {
        match std::env::var_os("MXRS_DIAGRAM_STATE_ROOT").filter(|root| !root.is_empty()) {
            Some(root) => PathBuf::from(root),
            None => std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".local/state/mxrs/diagram-er"),
        }
    }

    /// Starts the editor detached, and answers once it serves.
    pub fn up(
        &self,
        output: Option<&Path>,
        modules: Option<Vec<String>>,
        port: Option<u16>,
        force: bool,
    ) -> Result<Status, String> {
        let previous = self.read_state()?;
        if let Some(previous) = &previous {
            if self.running(previous, None) {
                return Err(format!("diagram already running at {}", url_for(previous)));
            }
            if starting(previous) {
                return Err("managed diagram is already starting".to_string());
            }
        }
        let selected_output = match (output, &previous) {
            (Some(output), _) => absolute(output)?,
            (None, Some(previous)) => PathBuf::from(text(previous, "output")),
            (None, None) => default_output(&self.source),
        };
        if let (Some(_), Some(previous)) = (output, &previous)
            && selected_output != Path::new(text(previous, "output"))
        {
            return Err(
                "destroy the existing managed diagram before changing --output".to_string(),
            );
        }
        let modules = modules.unwrap_or_else(|| {
            previous
                .as_ref()
                .and_then(|state| state["modules"].as_array())
                .map(|modules| {
                    modules
                        .iter()
                        .filter_map(|module| module.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        });
        let port = port
            .or_else(|| {
                previous
                    .as_ref()
                    .and_then(|state| state["port"].as_u64())
                    .and_then(|port| u16::try_from(port).ok())
            })
            .unwrap_or(DEFAULT_PORT);
        let mut managed: Vec<PathBuf> = previous
            .as_ref()
            .and_then(|state| state["managed_paths"].as_array())
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(|path| path.as_str().map(PathBuf::from))
                    .collect()
            })
            .unwrap_or_default();
        if previous.is_some() && selected_output.is_file() && !force {
            Server::new(
                &self.source,
                Some(&selected_output),
                Vec::new(),
                port,
                Output::Reuse,
                None,
            )?;
        } else {
            let server = Server::new(
                &self.source,
                Some(&selected_output),
                Vec::new(),
                port,
                Output::Fresh { force },
                None,
            )?;
            for path in server.managed_paths() {
                if !managed.contains(path) {
                    managed.push(path.clone());
                }
            }
        }
        let token = random_token();
        let state = json!({
            "version": 1,
            "source": self.source.display().to_string(),
            "token": token,
            "state": "starting",
            "pid": null,
            "log": self.log_path().display().to_string(),
            "managed_paths": managed.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "created_at": now(),
            "output": selected_output.display().to_string(),
            "modules": modules,
            "port": port,
            "host": "127.0.0.1",
        });
        self.write_state(&state)?;
        let started = self.spawn_worker(&state).and_then(|mut child| {
            let mut state = self.read_state()?.unwrap_or(state.clone());
            state["pid"] = json!(child.id());
            self.write_state(&state)?;
            let mut exited = None;
            let waited = self.wait_until(START_TIMEOUT, || {
                if let Ok(Some(status)) = child.try_wait() {
                    exited = Some(status);
                    return true;
                }
                self.read_state()
                    .ok()
                    .flatten()
                    .is_some_and(|state| self.running(&state, Some(&token)))
            });
            match exited {
                // A worker that stopped before it served says why in its log.
                Some(status) => Err(format!(
                    "the diagram server stopped before it served ({status}):\n{}",
                    self.log_tail()
                )),
                None => waited,
            }
        });
        if let Err(error) = started {
            self.mark_stopped(&token);
            return Err(error);
        }
        self.status()
    }

    pub fn status(&self) -> Result<Status, String> {
        let Some(state) = self.read_state()? else {
            return Ok(self.absent());
        };
        let lifecycle = if self.running(&state, None) {
            "running"
        } else if starting(&state) {
            "starting"
        } else {
            "stopped"
        };
        Ok(Status {
            state: lifecycle.to_string(),
            source: self.source.display().to_string(),
            output: state["output"].as_str().map(str::to_string),
            url: Some(url_for(&state)),
            pid: state["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok()),
            log: state["log"].as_str().map(str::to_string),
        })
    }

    /// Stops a running editor through its endpoint.
    pub fn down(&self) -> Result<Status, String> {
        let Some(state) = self.read_state()? else {
            return Err(format!("no managed diagram for {}", self.source.display()));
        };
        let token = text(&state, "token").to_string();
        if !self.running(&state, None) {
            if starting(&state) {
                return Err("managed diagram is still starting; retry in a moment".to_string());
            }
            self.mark_stopped(&token);
            return self.status();
        }
        if request(&state, "POST", "/api/admin/shutdown", &token) != Some(200) {
            return Err("managed diagram refused the shutdown request".to_string());
        }
        self.wait_until(STOP_TIMEOUT, || {
            !self
                .read_state()
                .ok()
                .flatten()
                .is_some_and(|state| self.running(&state, None))
        })?;
        self.mark_stopped(&token);
        self.status()
    }

    /// Stops the editor and removes its layout copy and state.
    pub fn destroy(&self, confirm: bool) -> Result<Status, String> {
        if !confirm {
            return Err("destroy requires --yes".to_string());
        }
        let Some(state) = self.read_state()? else {
            return Err(format!("no managed diagram for {}", self.source.display()));
        };
        let current = self.status()?;
        if matches!(current.state.as_str(), "running" | "starting") {
            self.down()?;
        }
        let output = PathBuf::from(text(&state, "output"));
        let paths: Vec<PathBuf> = state["managed_paths"]
            .as_array()
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(|path| path.as_str().map(PathBuf::from))
                    .collect()
            })
            .unwrap_or_default();
        for path in paths.iter().rev() {
            self.remove_managed_path(path, &output)?;
        }
        let instance = self.instance_dir();
        if instance.exists() {
            std::fs::remove_dir_all(&instance)
                .map_err(|error| format!("{}: {error}", instance.display()))?;
        }
        Ok(self.absent())
    }

    /// Serves as the managed editor `up` started: the worker's own entry.
    pub fn run_worker(
        &self,
        output: &Path,
        modules: Vec<String>,
        port: u16,
        token: &str,
    ) -> Result<(), String> {
        let state = self
            .read_state()?
            .ok_or_else(|| "diagram lifecycle state is missing".to_string())?;
        if text(&state, "token") != token {
            return Err("diagram lifecycle token does not match".to_string());
        }
        let server = Server::new(
            &self.source,
            Some(output),
            modules,
            port,
            Output::Reuse,
            Some(token.to_string()),
        )?;
        let served = server.serve(|_| {
            let mut running = state.clone();
            running["pid"] = json!(std::process::id());
            running["state"] = json!("running");
            running["started_at"] = json!(now());
            let _ = self.write_state(&running);
        });
        self.mark_stopped(token);
        served
    }

    fn absent(&self) -> Status {
        Status {
            state: "absent".to_string(),
            source: self.source.display().to_string(),
            output: None,
            url: None,
            pid: None,
            log: None,
        }
    }

    fn spawn_worker(&self, state: &Value) -> Result<std::process::Child, String> {
        self.prepare_instance_dir()?;
        let log_path = self.log_path();
        let log = std::fs::File::create(&log_path)
            .map_err(|error| format!("{}: {error}", log_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
        }
        let errors = log.try_clone().map_err(|error| error.to_string())?;
        let mut command = std::process::Command::new(&self.executable);
        command
            .args(["diagram-er", "__serve"])
            .arg(&self.source)
            .args(["--output", text(state, "output")])
            .args(["--port", &state["port"].to_string()])
            .env(LIFECYCLE_TOKEN_VARIABLE, text(state, "token"))
            .arg("--state-root")
            .arg(&self.state_root)
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(errors);
        for module in state["modules"].as_array().into_iter().flatten() {
            command.args(["--module", module.as_str().unwrap_or_default()]);
        }
        command
            .spawn()
            .map_err(|error| format!("cannot start the diagram server: {error}"))
    }

    /// The last lines the worker logged.
    fn log_tail(&self) -> String {
        let log = std::fs::read_to_string(self.log_path()).unwrap_or_default();
        let lines: Vec<&str> = log.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    }

    fn running(&self, state: &Value, token: Option<&str>) -> bool {
        let token = token.unwrap_or_else(|| text(state, "token"));
        !token.is_empty() && request(state, "GET", "/api/admin/status", token) == Some(200)
    }

    fn wait_until(&self, timeout: Duration, mut done: impl FnMut() -> bool) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if done() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "diagram lifecycle timed out; see {}",
                    self.log_path().display()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn mark_stopped(&self, token: &str) {
        let Ok(Some(mut state)) = self.read_state() else {
            return;
        };
        if text(&state, "token") != token {
            return;
        }
        state["pid"] = Value::Null;
        state["state"] = json!("stopped");
        state["stopped_at"] = json!(now());
        let _ = self.write_state(&state);
    }

    fn remove_managed_path(&self, path: &Path, output: &Path) -> Result<(), String> {
        let path = absolute(path)?;
        let output = absolute(output)?;
        let external = output
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("mprcontents");
        let source_contents = self
            .source
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("mprcontents");
        if path == self.source || path == source_contents {
            return Err("refusing to remove the source model".to_string());
        }
        // The folder made for the copy goes once it is empty.
        if Some(path.as_path()) == output.parent() {
            if path
                .read_dir()
                .is_ok_and(|mut entries| entries.next().is_none())
            {
                std::fs::remove_dir(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            }
            return Ok(());
        }
        if path != output && path != external {
            return Err(format!(
                "refusing to remove unmanaged path {}",
                path.display()
            ));
        }
        if path.is_dir() && !path.is_symlink() {
            std::fs::remove_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))
        } else if path.exists() || path.is_symlink() {
            std::fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))
        } else {
            Ok(())
        }
    }

    fn read_state(&self) -> Result<Option<Value>, String> {
        match std::fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("invalid diagram lifecycle state: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{}: {error}", self.state_path().display())),
        }
    }

    fn write_state(&self, state: &Value) -> Result<(), String> {
        self.prepare_instance_dir()?;
        let path = self.state_path();
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temporary, &path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn prepare_instance_dir(&self) -> Result<(), String> {
        let instance = self.instance_dir();
        std::fs::create_dir_all(&instance)
            .map_err(|error| format!("{}: {error}", instance.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&instance, std::fs::Permissions::from_mode(0o700));
        }
        Ok(())
    }

    fn instance_dir(&self) -> PathBuf {
        let digest = Sha256::digest(self.source.display().to_string().as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.state_root.join(&hex[..24])
    }

    fn state_path(&self) -> PathBuf {
        self.instance_dir().join("state.json")
    }

    fn log_path(&self) -> PathBuf {
        self.instance_dir().join("server.log")
    }
}

fn text<'a>(state: &'a Value, key: &str) -> &'a str {
    state[key].as_str().unwrap_or_default()
}

fn url_for(state: &Value) -> String {
    format!(
        "http://{}:{}",
        state["host"].as_str().unwrap_or("127.0.0.1"),
        state["port"]
    )
}

/// Whether a `starting` state is still young enough to be starting.
fn starting(state: &Value) -> bool {
    text(state, "state") == "starting"
        && state["created_at"]
            .as_u64()
            .is_some_and(|created| now().saturating_sub(created) <= START_TIMEOUT.as_secs())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The status code a managed server answers `path` with, or none.
fn request(state: &Value, method: &str, path: &str, token: &str) -> Option<u16> {
    let host = state["host"].as_str().unwrap_or("127.0.0.1");
    let port = u16::try_from(state["port"].as_u64()?).ok()?;
    let address = std::net::SocketAddr::new(host.parse().ok()?, port);
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(250)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n{LIFECYCLE_TOKEN}: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn model(directory: &Path) -> PathBuf {
        let path = directory.join("App.mpr");
        let mut project = mxrs_dsl::ProjectBuilder::new("11.12.1");
        project.module("Sales", |module| {
            module.entity("Order", |entity| {
                entity.string("Number");
            });
        });
        mxrs_writer::write_project(&path, &project.build()).unwrap();
        path
    }

    /// Every file under `folder`, by its path, with its bytes.
    fn contents_of(folder: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![folder.to_path_buf()];
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
        assert!(!files.is_empty());
        files
    }

    /// One request to the server: its status and body.
    fn ask(
        port: u16,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let headers: String = headers
            .iter()
            .map(|(key, value)| format!("{key}: {value}\r\n"))
            .collect();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let status = answer.split_whitespace().nth(1).unwrap().parse().unwrap();
        let body = answer.split_once("\r\n\r\n").unwrap().1.to_string();
        (status, body)
    }

    /// The editor works on a copy: it serves the page and its assets, the
    /// diagram with the token a layout is signed with, takes a signed
    /// layout into the copy only, and keeps its lifecycle endpoints to the
    /// holder of the lifecycle token.
    #[test]
    fn the_editor_serves_the_diagram_and_takes_signed_layouts_into_its_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = model(directory.path());
        let before = std::fs::read(&source).unwrap();
        let server = Server::new(
            &source,
            None,
            Vec::new(),
            0,
            Output::Fresh { force: false },
            Some("lifecycle".to_string()),
        )
        .unwrap();
        // A v2 model's copy is in a folder of its own, with contents of
        // its own.
        assert_eq!(
            server.output(),
            directory
                .path()
                .join("App.domain-layout/App.domain-layout.mpr")
        );
        let source_contents = contents_of(&directory.path().join("mprcontents"));
        let (sender, receiver) = std::sync::mpsc::channel();
        let serving = std::thread::spawn(move || server.serve(|port| sender.send(port).unwrap()));
        let port = receiver.recv().unwrap();

        let (status, page) = ask(port, "GET", "/", &[], "");
        assert_eq!(status, 200);
        assert!(page.contains("<title>MXRS · Domain Model</title>"));
        assert_eq!(
            ask(port, "GET", "/assets/domain-mGOZ1KCG.css", &[], "").0,
            200
        );
        assert_eq!(ask(port, "GET", "/assets/../domain.html", &[], "").0, 404);
        let (status, diagram) = ask(port, "GET", "/api/diagram", &[], "");
        assert_eq!(status, 200);
        let diagram: Value = serde_json::from_str(&diagram).unwrap();
        let token = diagram["csrf_token"].as_str().unwrap().to_string();
        let module = diagram["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|module| module["name"] == "Sales")
            .unwrap()
            .clone();
        let entity = module["entities"][0].clone();
        let layout = json!({
            "modules": [{
                "name": "Sales",
                "entities": [{ "id": entity["id"], "x": 640, "y": 320 }],
                "associations": [],
            }],
        })
        .to_string();
        let (status, refused) = ask(port, "POST", "/api/layout", &[], &layout);
        assert_eq!(status, 422, "{refused}");
        assert!(refused.contains("invalid editor token"));
        let (status, applied) = ask(
            port,
            "POST",
            "/api/layout",
            &[("X-MXRB-Token", &token)],
            &layout,
        );
        assert_eq!(status, 200, "{applied}");
        assert_eq!(std::fs::read(&source).unwrap(), before);
        assert_eq!(
            contents_of(&directory.path().join("mprcontents")),
            source_contents
        );

        assert_eq!(ask(port, "GET", "/api/admin/status", &[], "").0, 403);
        // A page reaching the port through another name is refused.
        let mut rebound = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            rebound,
            "GET /api/diagram HTTP/1.1\r\nHost: attacker.example:{port}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut answer = String::new();
        rebound.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
        assert_eq!(
            ask(
                port,
                "GET",
                "/api/admin/status",
                &[("X-MXRS-Lifecycle-Token", "lifecycle")],
                ""
            )
            .0,
            200
        );
        assert_eq!(
            ask(
                port,
                "POST",
                "/api/admin/shutdown",
                &[("X-MXRS-Lifecycle-Token", "lifecycle")],
                ""
            )
            .0,
            200
        );
        serving.join().unwrap().unwrap();
    }

    /// The copy is never the model, and an existing copy is replaced only
    /// when forced.
    #[test]
    fn the_layout_copy_is_never_the_model() {
        let directory = tempfile::tempdir().unwrap();
        let source = model(directory.path());
        let fresh = Output::Fresh { force: false };
        assert!(
            Server::new(&source, Some(&source), Vec::new(), 0, fresh, None)
                .err()
                .unwrap()
                .contains("safe copy")
        );
        Server::new(&source, None, Vec::new(), 0, fresh, None).unwrap();
        assert!(
            Server::new(&source, None, Vec::new(), 0, fresh, None)
                .err()
                .unwrap()
                .contains("--force")
        );
        Server::new(
            &source,
            None,
            Vec::new(),
            0,
            Output::Fresh { force: true },
            None,
        )
        .unwrap();
        assert!(
            Server::new(
                directory.path().join("Missing.mpr"),
                None,
                Vec::new(),
                0,
                fresh,
                None
            )
            .err()
            .unwrap()
            .contains("MPR not found")
        );
    }
}
