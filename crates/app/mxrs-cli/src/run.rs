//! `mxrs run` — boot the project's model on MXRS' own runtime.
//!
//! Ports `Mxrb::RubyApp::Supervisor`'s contract (host/ports/environment
//! flags, optional Vite frontend, terminate-on-interrupt, "frontend process
//! exited with …" failures) onto a different backend: where mxrb starts the
//! generated Ruby application server, mxrs boots its own runtime in-process —
//! [`mxrs_runtime_boot`] derives the store schema and security policy from
//! the built `.mpr`, state persists through SQLite under `.mxrs/runtime/`,
//! and [`mxrs_runtime_http::RuntimeHttp`] serves the built web shell.
//!
//! Every named flow is registered on [`mxrs_runtime_flows::FlowEngine`] —
//! the native interpreter — so `POST /api/microflow/<Module.Flow>` executes
//! the model's own logic inside `Runtime::invoke`'s transaction, returning
//! the result plus the client effects and log the flow emitted.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex};

use mxrs_runtime::{Runtime, Store};
use mxrs_runtime_http::RuntimeHttp;
use mxrs_runtime_sqlite::SqliteRuntimeStore;

use crate::environment::{EnvironmentError, EnvironmentProfile};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("{0}")]
    Target(String),
    #[error(transparent)]
    Environment(#[from] EnvironmentError),
    #[error(transparent)]
    Boot(#[from] mxrs_runtime_boot::BootError),
    #[error("cannot persist runtime state: {0}")]
    Persistence(#[from] mxrs_runtime_sqlite::SqliteRuntimeError),
    #[error(transparent)]
    Http(#[from] mxrs_runtime_http::HttpError),
    #[error("{0}")]
    Frontend(String),
    #[error("invalid scheduled event: {0}")]
    Scheduler(String),
    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub root: PathBuf,
    pub host: String,
    pub server_port: u16,
    pub client_port: u16,
    pub frontend: bool,
    pub environment: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunTarget {
    pub mpr: PathBuf,
    pub web_root: PathBuf,
    pub state_path: PathBuf,
}

/// Locates the built model and web shell under a generated project root.
/// `cargo mxrs build --output build/<name>.mpr` produces both.
pub fn resolve_target(root: &Path) -> Result<RunTarget, RunError> {
    let build = root.join("build");
    let mut models: Vec<PathBuf> = std::fs::read_dir(&build)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mpr"))
        .collect();
    models.sort();
    let mpr = match models.len() {
        0 => {
            return Err(RunError::Target(format!(
                "no built model under {}; run `cargo mxrs build --output build/<name>.mpr` first",
                build.display()
            )));
        }
        1 => models.remove(0),
        _ => {
            return Err(RunError::Target(format!(
                "multiple .mpr files under {}; keep exactly one built model",
                build.display()
            )));
        }
    };
    let web_root = build.join("web");
    if !web_root.is_dir() {
        return Err(RunError::Target(format!(
            "no web assets under {}; `cargo mxrs build` materializes them",
            web_root.display()
        )));
    }
    Ok(RunTarget {
        mpr,
        web_root,
        state_path: root.join(".mxrs").join("runtime").join("state.sqlite3"),
    })
}

/// The frontend dev server's extra environment: the profile's `VITE_`
/// passthrough plus the same markers mxrb sets (`MXRB_ENV`/`MXRB_API_PORT`
/// there, `MXRS_`-prefixed here).
pub fn frontend_environment(profile: &EnvironmentProfile, api_port: u16) -> Vec<(String, String)> {
    let mut variables: Vec<(String, String)> = profile
        .variables()
        .filter(|(key, _)| key.starts_with("VITE_"))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    variables.push(("MXRS_ENV".to_string(), profile.environment.clone()));
    variables.push(("MXRS_API_PORT".to_string(), api_port.to_string()));
    variables
}

/// The command line the frontend starts with, after mxrb's own preflight:
/// the package must exist and its dependencies must be installed.
pub fn frontend_command(root: &Path, options: &RunOptions) -> Result<Command, RunError> {
    let directory = root.join("frontend");
    if !directory.join("package.json").is_file() {
        return Err(RunError::Frontend(format!(
            "frontend package not found: {}",
            directory.display()
        )));
    }
    let npm = std::env::var("MXRS_NPM").unwrap_or_else(|_| "npm".to_string());
    if !directory.join("node_modules").is_dir() {
        return Err(RunError::Frontend(format!(
            "frontend dependencies missing; run `{npm} install --prefix frontend`"
        )));
    }
    let mut command = Command::new(npm);
    command
        .args(["run", "dev", "--"])
        .args(["--host", &options.host])
        .args(["--port", &options.client_port.to_string()])
        .arg("--strictPort")
        .current_dir(&directory);
    Ok(command)
}

fn bind_address(options: &RunOptions) -> Result<SocketAddr, RunError> {
    let ip: IpAddr = match options.host.as_str() {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        host => host.parse().map_err(|_| {
            RunError::Target(format!(
                "invalid host {host:?}; use an IP address or localhost"
            ))
        })?,
    };
    Ok(SocketAddr::new(ip, options.server_port))
}

/// One model flow exposed as a runtime action: `Runtime::invoke` owns the
/// document authorization and the transaction; the engine executes inside
/// that unit of work and returns the result plus client effects and log.
struct FlowAction {
    engine: Arc<mxrs_runtime_flows::FlowEngine>,
    name: String,
    context: mxrs_runtime::SecurityContext,
}

impl mxrs_runtime::Action for FlowAction {
    fn execute(
        &self,
        store: &mut Store,
        arguments: &serde_json::Value,
    ) -> mxrs_runtime::Result<serde_json::Value> {
        let mut variables = mxrs_runtime_flows::Variables::new();
        if let serde_json::Value::Object(map) = arguments {
            for (name, value) in map {
                variables.insert(
                    name.clone(),
                    mxrs_runtime_flows::FlowValue::from_member(value),
                );
            }
        }
        let mut execution = self.engine.new_execution(Some(self.context.clone()));
        match self
            .engine
            .call_in_unit(store, &mut execution, &self.name, variables)
        {
            Ok(value) => Ok(serde_json::json!({
                "result": value.to_json_shallow(),
                "effects": execution.effects,
                "log": execution.log,
            })),
            Err(mxrs_runtime_flows::FlowError::Runtime(error)) => Err(error),
            Err(mxrs_runtime_flows::FlowError::Native(message)) => {
                Err(mxrs_runtime::RuntimeError::Transaction(message))
            }
        }
    }
}

/// What stopped the server: the interrupt signal, or the frontend process
/// exiting on its own (mxrb's Supervisor waits on the frontend the same way).
enum Stop {
    Interrupt,
    Frontend(Option<ExitStatus>),
}

pub fn start(options: &RunOptions) -> Result<(), RunError> {
    let target = resolve_target(&options.root)?;
    let address = bind_address(options)?;
    let profile = EnvironmentProfile::load(&options.root, options.environment.as_deref())?;
    let boot = mxrs_runtime_boot::boot(&target.mpr)?;

    let state_directory = target
        .state_path
        .parent()
        .expect("state path always has a parent");
    std::fs::create_dir_all(state_directory).map_err(|source| RunError::Io {
        path: state_directory.display().to_string(),
        source,
    })?;
    let mut persistence = SqliteRuntimeStore::open(&target.state_path)?;
    let mut store = Store::new(boot.schema.clone());
    let restored = persistence
        .load(&mut store)
        .map_err(RunError::Persistence)?;
    let mut runtime = Runtime::new(store, boot.security.clone());
    let engine = Arc::new(
        mxrs_runtime_flows::FlowEngine::from_modules(&boot.modules)
            .with_policy(boot.security.clone()),
    );
    let flow_names: Vec<String> = engine.flow_names().map(str::to_string).collect();
    for name in &flow_names {
        runtime.register_action(
            name.clone(),
            FlowAction {
                engine: engine.clone(),
                name: name.clone(),
                context: mxrs_runtime::SecurityContext::default(),
            },
        );
    }
    let http = RuntimeHttp::new(runtime, &target.web_root);
    let runtime_handle = http.runtime_handle();
    let jobs = mxrs_runtime_scheduler::jobs_from_modules(&boot.modules)
        .map_err(|error| RunError::Scheduler(error.to_string()))?;
    let scheduler = (!jobs.is_empty()).then(|| SchedulerTask {
        scheduler: mxrs_runtime_scheduler::Scheduler::new(jobs),
        engine: engine.clone(),
        runtime: runtime_handle.clone(),
    });

    println!("[mxrs] Environment: {}", profile.environment);
    println!(
        "[mxrs] Booted {} entities and {} flow documents from {} ({} object(s) restored)",
        boot.entities,
        boot.documents,
        target.mpr.display(),
        restored,
    );
    if !boot.skipped_xpath_rules.is_empty() {
        println!(
            "[mxrs] warning: {} XPath-guarded access rule(s) enforced as deny until the XPath engine is ported",
            boot.skipped_xpath_rules.len()
        );
    }
    println!(
        "[mxrs] {} flow(s) registered on the native interpreter (POST /api/microflow/<Module.Flow>)",
        flow_names.len()
    );
    if let Some(task) = &scheduler {
        println!(
            "[mxrs] {} scheduled event(s) armed (1s poll)",
            task.scheduler.jobs().len()
        );
    }
    println!(
        "[mxrs] Runtime server: http://{}:{}",
        options.host, options.server_port
    );
    if options.frontend {
        println!(
            "[mxrs] React client: http://{}:{}",
            options.host, options.client_port
        );
    }

    let frontend = if options.frontend {
        let mut command = frontend_command(&options.root, options)?;
        command.envs(frontend_environment(&profile, options.server_port));
        let child = command.spawn().map_err(|source| {
            RunError::Frontend(format!("cannot start the frontend dev server: {source}"))
        })?;
        Some(child)
    } else {
        None
    };

    let outcome = serve(http, address, frontend, scheduler);
    let store_snapshot = runtime_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .store()
        .clone();
    let persisted = persistence.save(&store_snapshot);
    outcome?;
    persisted?;
    Ok(())
}

/// The scheduler's runtime wiring: every tick locks the runtime and executes
/// due flows on the interpreter as system calls (no security context — mxrb's
/// scheduler executor does the same).
struct SchedulerTask {
    scheduler: mxrs_runtime_scheduler::Scheduler,
    engine: Arc<mxrs_runtime_flows::FlowEngine>,
    runtime: Arc<Mutex<mxrs_runtime::Runtime>>,
}

impl SchedulerTask {
    async fn run(mut self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs_f64())
                .unwrap_or(0.0);
            let engine = self.engine.clone();
            let runtime = self.runtime.clone();
            let mut executor = move |flow: &str| {
                let mut runtime = runtime
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                engine
                    .call(
                        runtime.store_mut(),
                        flow,
                        mxrs_runtime_flows::Variables::new(),
                        None,
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            };
            match self.scheduler.tick(now, &mut executor) {
                Ok(_) => {}
                Err(error) => {
                    // mxrb's run loop stops on a scheduler failure after
                    // logging it; ticking on would repeat the same error
                    // every second.
                    eprintln!("[mxrs] scheduler failed: {error}");
                    return;
                }
            }
            for (job, message) in self.scheduler.errors.drain(..) {
                eprintln!("[mxrs] scheduled event {job} failed: {message}");
            }
        }
    }
}

fn serve(
    http: RuntimeHttp,
    address: SocketAddr,
    frontend: Option<Child>,
    scheduler: Option<SchedulerTask>,
) -> Result<(), RunError> {
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| RunError::Io {
            path: "tokio runtime".to_string(),
            source,
        })?;
    let (frontend_pid, frontend_exit) = match frontend {
        Some(mut child) => {
            let pid = child.id();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let status = child.wait().ok();
                let _ = sender.send(status);
            });
            (Some(pid), Some(receiver))
        }
        None => (None, None),
    };
    let stop_reason: Arc<Mutex<Option<Stop>>> = Arc::new(Mutex::new(None));
    let shutdown_reason = stop_reason.clone();
    let shutdown = async move {
        let stop = match frontend_exit {
            Some(receiver) => tokio::select! {
                _ = tokio::signal::ctrl_c() => Stop::Interrupt,
                status = receiver => Stop::Frontend(status.ok().flatten()),
            },
            None => {
                let _ = tokio::signal::ctrl_c().await;
                Stop::Interrupt
            }
        };
        *shutdown_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stop);
    };
    // The server outcome is held, not propagated: a bind failure must still
    // terminate an already-spawned frontend (mxrb's `ensure shutdown`), or a
    // failed `mxrs run` leaks a Vite process holding the client port.
    let served = tokio_runtime.block_on(async {
        let ticker = scheduler.map(|task| tokio::spawn(task.run()));
        let served = http.serve_with_shutdown(address, shutdown).await;
        if let Some(ticker) = ticker {
            ticker.abort();
        }
        served
    });
    let stop = stop_reason
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let outcome = match stop {
        Some(Stop::Frontend(status)) => match status {
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(RunError::Frontend(frontend_exit_message(status))),
            None => Err(RunError::Frontend(
                "frontend process could not be awaited".to_string(),
            )),
        },
        _ => {
            // Interrupt or a server-side stop: terminate the frontend the
            // way mxrb does, TERM then reap. std's `Child::kill` sends
            // SIGKILL, so the graceful signal goes through the system `kill`.
            if let Some(pid) = frontend_pid {
                let _ = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
            }
            Ok(())
        }
    };
    served?;
    outcome
}

fn frontend_exit_message(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("frontend process exited with status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("frontend process exited with signal {signal}");
        }
    }
    "frontend process exited with a signal".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(root: &Path) -> RunOptions {
        RunOptions {
            root: root.to_path_buf(),
            host: "127.0.0.1".to_string(),
            server_port: 9292,
            client_port: 5173,
            frontend: true,
            environment: None,
        }
    }

    #[test]
    fn resolving_a_target_requires_exactly_one_built_model_and_web_assets() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let missing = resolve_target(root).unwrap_err().to_string();
        assert!(missing.contains("no built model"), "{missing}");
        assert!(missing.contains("cargo mxrs build"), "{missing}");

        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build/app.mpr"), b"").unwrap();
        let no_web = resolve_target(root).unwrap_err().to_string();
        assert!(no_web.contains("no web assets"), "{no_web}");

        std::fs::create_dir_all(root.join("build/web")).unwrap();
        let target = resolve_target(root).unwrap();
        assert_eq!(target.mpr, root.join("build/app.mpr"));
        assert_eq!(target.web_root, root.join("build/web"));
        assert_eq!(
            target.state_path,
            root.join(".mxrs").join("runtime").join("state.sqlite3")
        );

        std::fs::write(root.join("build/other.mpr"), b"").unwrap();
        let ambiguous = resolve_target(root).unwrap_err().to_string();
        assert!(ambiguous.contains("multiple .mpr files"), "{ambiguous}");
    }

    #[test]
    fn the_frontend_preflight_matches_mxrb_and_the_command_pins_the_port() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let options = options(root);

        let missing = frontend_command(root, &options).unwrap_err().to_string();
        assert!(missing.contains("frontend package not found"), "{missing}");

        std::fs::create_dir_all(root.join("frontend")).unwrap();
        std::fs::write(root.join("frontend/package.json"), b"{}").unwrap();
        let uninstalled = frontend_command(root, &options).unwrap_err().to_string();
        assert_eq!(
            uninstalled,
            "frontend dependencies missing; run `npm install --prefix frontend`"
        );

        std::fs::create_dir_all(root.join("frontend/node_modules")).unwrap();
        let command = frontend_command(root, &options).unwrap();
        let arguments: Vec<_> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            [
                "run",
                "dev",
                "--",
                "--host",
                "127.0.0.1",
                "--port",
                "5173",
                "--strictPort"
            ]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(root.join("frontend").as_path())
        );
    }

    #[test]
    fn the_frontend_environment_passes_vite_variables_and_the_runtime_markers() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(".env"),
            "VITE_API_LABEL=orders\nSECRET_TOKEN=never\n",
        )
        .unwrap();
        let profile = EnvironmentProfile::load(directory.path(), Some("development")).unwrap();
        let variables = frontend_environment(&profile, 9292);
        assert!(
            variables
                .iter()
                .any(|(key, value)| key == "VITE_API_LABEL" && value == "orders")
        );
        assert!(variables.iter().any(|(key, _)| key == "MXRS_ENV"));
        assert!(
            variables
                .iter()
                .any(|(key, value)| key == "MXRS_API_PORT" && value == "9292")
        );
        assert!(!variables.iter().any(|(key, _)| key == "SECRET_TOKEN"));
    }

    #[test]
    fn hosts_must_be_ip_addresses_or_localhost() {
        let directory = tempfile::tempdir().unwrap();
        let mut options = options(directory.path());
        assert!(bind_address(&options).is_ok());
        options.host = "localhost".to_string();
        assert_eq!(
            bind_address(&options).unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        options.host = "evil.example".to_string();
        assert!(bind_address(&options).is_err());
    }
}
