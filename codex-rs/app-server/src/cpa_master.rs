//! Credential-directory supervisor. Account code only runs in disposable children.
mod worker;
use anyhow::Context;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::routing::get;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::InitializeResponse;
use codex_login::default_client::USER_AGENT_SUFFIX;
use codex_login::default_client::get_codex_user_agent;
use codex_login::default_client::set_default_originator;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use worker::Worker;

#[derive(Clone)]
struct Client {
    business: mpsc::UnboundedSender<Value>,
    diagnostics: mpsc::UnboundedSender<Value>,
}

struct ClientMessages {
    business: mpsc::UnboundedReceiver<Value>,
    diagnostics: mpsc::UnboundedReceiver<Value>,
}

impl Client {
    fn channel() -> (Self, ClientMessages) {
        let (business, business_rx) = mpsc::unbounded_channel();
        let (diagnostics, diagnostics_rx) = mpsc::unbounded_channel();
        (
            Self {
                business,
                diagnostics,
            },
            ClientMessages {
                business: business_rx,
                diagnostics: diagnostics_rx,
            },
        )
    }

    async fn send(&self, message: Value) -> Result<(), mpsc::error::SendError<Value>> {
        let sender = if matches!(
            message["method"].as_str(),
            Some("cpa/inference/upstream" | "cpa/inference/diagnostics/completed")
        ) {
            &self.diagnostics
        } else {
            &self.business
        };
        sender.send(message)
    }

    fn same_channel(&self, other: &Self) -> bool {
        self.business.same_channel(&other.business)
    }

    async fn diagnostics_completed(&self, request_id: &str) {
        let _ = self.send(json!({"method":"cpa/inference/diagnostics/completed","params":{"requestId":request_id}})).await;
    }
}

impl ClientMessages {
    async fn recv(&mut self) -> Option<Value> {
        // Prioritize queued business frames. A write already in progress on the
        // shared WebSocket still has to finish before another frame can be sent.
        tokio::select! {
            biased;
            Some(message) = self.business.recv() => Some(message),
            Some(message) = self.diagnostics.recv() => Some(message),
            else => None,
        }
    }
}

struct Master {
    shutdown: CancellationToken,
    root: PathBuf,
    state: PathBuf,
    workers: Mutex<HashMap<String, Arc<Worker>>>,
    logins: Mutex<HashMap<String, String>>,
}

/// Run the CPA master without initializing authentication, telemetry or agent state.
pub async fn run() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();
    let root = std::env::var_os("CODEX_CPA_AUTH_DIR").context("CODEX_CPA_AUTH_DIR required")?;
    let root = std::fs::canonicalize(root)?;
    let state =
        PathBuf::from(std::env::var_os("CODEX_HOME").unwrap_or_else(|| "/var/lib/codex".into()));
    tokio::fs::create_dir_all(&state).await?;
    let state = std::fs::canonicalize(state)?;
    let master = Arc::new(Master {
        shutdown: CancellationToken::new(),
        root,
        state,
        workers: Mutex::default(),
        logins: Mutex::default(),
    });
    master.reload(/*only*/ None).await?;
    let port: u16 = std::env::var("CODEX_CPA_PORT")
        .unwrap_or_else(|_| "38317".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let watcher = master.clone();
    let watch = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_millis(/*millis*/ 500));
        loop {
            tokio::select! { _ = watcher.shutdown.cancelled() => break, _ = timer.tick() => {} }
            if let Err(error) = watcher.reload(/*only*/ None).await {
                tracing::warn!(%error, "CPA scan failed");
            }
        }
    });
    let app = router(master.clone());
    let shutdown_master = master.clone();
    let shutdown = async move {
        let signals = async {
            #[cfg(unix)]
            {
                if let Ok(mut term) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
        };
        let parent_closed = async {
            if cfg!(debug_assertions) && std::env::var_os("CODEX_CPA_TEST_STDIN_LIFETIME").is_some()
            {
                use tokio::io::AsyncReadExt;
                let mut byte = [0];
                while tokio::io::stdin()
                    .read(&mut byte)
                    .await
                    .is_ok_and(|read| read != 0)
                {}
            } else {
                std::future::pending::<()>().await;
            }
        };
        tokio::select! { _ = signals => {}, _ = parent_closed => {} }
        shutdown_master.shutdown.cancel();
        watch.abort();
        let workers = std::mem::take(&mut *shutdown_master.workers.lock().await);
        for (_, worker) in workers {
            let _ = worker.stop().await;
        }
    };
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    result?;
    Ok(())
}

fn router(master: Arc<Master>) -> Router {
    Router::new()
        .route("/readyz", get(|| async { "ok" }))
        .route(
            "/cpa/v1/ws",
            get(
                |State(master): State<Arc<Master>>, upgrade: WebSocketUpgrade| async move {
                    // Axum requires concrete sizes; use the addressable maximum for this local bridge.
                    upgrade
                        .max_message_size(usize::MAX)
                        .max_frame_size(usize::MAX)
                        .on_upgrade(move |socket| master.connection(socket))
                },
            ),
        )
        .with_state(master)
}

impl Master {
    fn initialize_response(&self, params: InitializeParams) -> anyhow::Result<InitializeResponse> {
        let name = params.client_info.name;
        anyhow::ensure!(
            axum::http::HeaderValue::from_str(&name).is_ok(),
            "Invalid clientInfo.name: '{name}'. Must be a valid HTTP header value."
        );
        // Match the native initialize processor's originating-client rules. The
        // supervisor has no account/Agent state, but must use native identity APIs.
        if !matches!(name.as_str(), "codex_app_server_daemon" | "codex-backend") {
            if let Err(codex_login::default_client::SetOriginatorError::InvalidHeaderValue) =
                set_default_originator(name.clone())
            {
                anyhow::bail!("Invalid clientInfo.name: '{name}'");
            }
            if let Ok(mut suffix) = USER_AGENT_SUFFIX.lock() {
                *suffix = Some(format!("{name}; {}", params.client_info.version));
            }
        }
        Ok(InitializeResponse {
            user_agent: get_codex_user_agent(),
            codex_home: self.state.clone().try_into()?,
            platform_family: std::env::consts::FAMILY.into(),
            platform_os: std::env::consts::OS.into(),
        })
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Serialize process creation and reaping so one credential cannot acquire two runtimes"
    )]
    async fn reload(&self, only: Option<&str>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.shutdown.is_cancelled(), "Master is stopping");
        let mut workers = self.workers.lock().await;
        anyhow::ensure!(!self.shutdown.is_cancelled(), "Master is stopping");
        let enabled = scan(&self.root)?;
        let stop: Vec<_> = workers
            .keys()
            .filter(|id| only.is_none_or(|only| only == id.as_str()) && !enabled.contains_key(*id))
            .cloned()
            .collect();
        for id in stop {
            if let Some(worker) = workers.remove(&id) {
                worker.stop().await?;
            }
            self.logins
                .lock()
                .await
                .retain(|_, credential| credential != &id);
        }
        for id in enabled
            .keys()
            .filter(|id| only.is_none_or(|only| only == id.as_str()))
        {
            if !workers.contains_key(id) {
                let home = self.state.join(id);
                // Preserve an existing account's configuration and installation identity
                // when upgrading from the former opaque directory layout.
                let legacy_home = self
                    .state
                    .join(format!("{:x}", Sha256::digest(id.as_bytes())));
                if !home.try_exists()? && legacy_home.try_exists()? {
                    if let Some(parent) = home.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::rename(&legacy_home, &home).await?;
                }
                let worker = Worker::start(&self.root, &home, id).await?;
                workers.insert(id.clone(), worker);
            }
        }
        Ok(())
    }

    async fn connection(self: Arc<Self>, socket: WebSocket) {
        let (mut sink, mut source) = socket.split();
        let (client, mut messages) = Client::channel();
        let writer = tokio::spawn(async move {
            while let Some(message) = messages.recv().await {
                if sink
                    .send(Message::Text(message.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut initialized = false;
        let mut requests = tokio::task::JoinSet::new();
        loop {
            let next = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = requests.join_next(), if !requests.is_empty() => continue,
                message = source.next() => message,
            };
            let Some(Ok(message)) = next else { break };
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(message) = serde_json::from_str::<Value>(&text) else {
                let _ = client.send(error(Value::Null, 400, "Invalid JSON")).await;
                continue;
            };
            match message["method"].as_str() {
                Some("initialize") => {
                    let params =
                        serde_json::from_value::<InitializeParams>(message["params"].clone());
                    let response = match params {
                        Ok(params) => self.initialize_response(params),
                        Err(error) => Err(error.into()),
                    };
                    let response = match response {
                        Ok(response) => response,
                        Err(error) => {
                            let _ = client
                                .send(json!({"id":message["id"],"error":{
                                    "code":-32600,"message":error.to_string()
                                }}))
                                .await;
                            continue;
                        }
                    };
                    initialized = true;
                    let _ = client
                        .send(json!({"id":message["id"],"result":response}))
                        .await;
                }
                Some("initialized") => {}
                _ if !initialized => {
                    let _ = client
                        .send(error(message["id"].clone(), 400, "Initialize first"))
                        .await;
                }
                _ => {
                    let master = self.clone();
                    let client = client.clone();
                    requests.spawn(async move {
                        let id = message["id"].clone();
                        let credential_id = message["params"]["credentialId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        let request_id = message["params"]["requestId"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        let inference = message["method"] == "cpa/inference/start";
                        if let Err(err) = master.dispatch(message, &client).await {
                            let details = json!({"error": err.to_string(), "rpcId": id});
                            let log_request_id = request_id.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::cpa_bridge::record_request(
                                    &credential_id,
                                    &log_request_id,
                                    "cpa.master.error",
                                    details,
                                );
                            });
                            let _ = client.send(error(id, 503, &err.to_string())).await;
                            if inference {
                                let workers: Vec<_> =
                                    master.workers.lock().await.values().cloned().collect();
                                let mut active = false;
                                for worker in workers {
                                    active |= worker.owns_request(&request_id, &client).await;
                                }
                                // A duplicate start on the same connection must not
                                // terminate the original request's diagnostic FIFO.
                                if !active {
                                    client.diagnostics_completed(&request_id).await;
                                }
                            }
                        }
                    });
                }
            }
        }
        requests.abort_all();
        while requests.join_next().await.is_some() {}
        let workers: Vec<_> = self.workers.lock().await.values().cloned().collect();
        for worker in workers {
            worker.disconnect(&client).await;
        }
        writer.abort();
    }

    async fn dispatch(&self, message: Value, client: &Client) -> anyhow::Result<()> {
        let method = message["method"].as_str().context("method required")?;
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let mut response = match method {
            "cpa/capabilities/read" => json!({"result":crate::cpa_bridge::capabilities()}),
            "cpa/credential/reload" => {
                let params: codex_app_server_protocol::CpaCredentialReloadParams =
                    serde_json::from_value(params)?;
                self.reload(params.credential_id.as_deref()).await?;
                json!({"result":crate::cpa_bridge::capabilities()})
            }
            "cpa/inference/start" | "cpa/auth/login/start" => {
                if method == "cpa/inference/start" {
                    let _: codex_app_server_protocol::CpaInferenceStartParams =
                        serde_json::from_value(params.clone())?;
                }
                let id = params["credentialId"]
                    .as_str()
                    .context("credentialId required")?;
                self.reload(Some(id)).await?;
                let worker = self
                    .workers
                    .lock()
                    .await
                    .get(id)
                    .cloned()
                    .context("Credential is disabled or missing")?;
                if method == "cpa/inference/start" {
                    worker.forward(message, client.clone()).await?;
                    return Ok(());
                }
                let response = worker.rpc(method, params.clone()).await?;
                if let Some(login) = response["result"]["loginId"].as_str() {
                    self.logins
                        .lock()
                        .await
                        .retain(|_, credential| credential != id);
                    self.logins.lock().await.insert(login.into(), id.into());
                }
                response
            }
            "cpa/auth/login/callback" | "cpa/auth/login/status" => {
                let login = params["loginId"].as_str().context("loginId required")?;
                let id = self
                    .logins
                    .lock()
                    .await
                    .get(login)
                    .cloned()
                    .context("Unknown loginId")?;
                let worker = self
                    .workers
                    .lock()
                    .await
                    .get(&id)
                    .cloned()
                    .context("Credential is stopped")?;
                worker.rpc(method, params).await?
            }
            "cpa/inference/cancel" => {
                let request_id = params["requestId"].as_str().context("requestId required")?;
                let workers: Vec<_> = self.workers.lock().await.values().cloned().collect();
                for worker in workers {
                    if worker.owns_request(request_id, client).await {
                        worker.forward(message, client.clone()).await?;
                        return Ok(());
                    }
                }
                json!({"result":{}})
            }
            _ => json!({"error":{"code":-32601,"message":"Method not available in CPA mode"}}),
        };
        response["id"] = message["id"].clone();
        client.send(response).await?;
        Ok(())
    }
}

fn scan(root: &Path) -> anyhow::Result<HashMap<String, PathBuf>> {
    let mut directories = vec![root.to_path_buf()];
    let mut enabled = HashMap::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                directories.push(entry.path());
            }
            if !kind.is_file()
                || entry
                    .path()
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let path = entry.path();
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if value["type"]
                .as_str()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("codex"))
                && value["codex_cli"]["enabled"] == true
                && value["disabled"] != true
            {
                let id = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("Credential ID is not UTF-8")?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                enabled.insert(id, path);
            }
        }
    }
    Ok(enabled)
}

fn error(id: Value, status: u16, message: &str) -> Value {
    json!({"id":id,"error":{"code":-32000,"message":message,"data":{"httpStatus":status}}})
}

#[cfg(test)]
#[path = "cpa_master_tests.rs"]
mod tests;
