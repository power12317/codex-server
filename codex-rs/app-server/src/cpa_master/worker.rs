//! One native account runtime. The master never initializes account auth clients.
use super::*;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStdin;
use tokio::process::Command;
use tokio::sync::oneshot;

enum Pending {
    Reply(oneshot::Sender<Value>),
    Forward {
        id: Value,
        client: Client,
        request_id: Option<String>,
    },
}

pub(super) struct Worker {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    sequence: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Pending>>>,
    streams: Arc<Mutex<HashMap<String, Client>>>,
}

impl Worker {
    pub(super) async fn start(root: &Path, home: &Path, id: &str) -> anyhow::Result<Arc<Self>> {
        tokio::fs::create_dir_all(home).await?;
        let args = {
            let mut args = Vec::new();
            let mut source = std::env::args_os().skip(/*n*/ 1);
            while let Some(arg) = source.next() {
                if arg == "--listen" {
                    source.next();
                } else if !arg.to_string_lossy().starts_with("--listen=") {
                    args.push(arg);
                }
            }
            args
        };
        let mut child = Command::new(std::env::current_exe()?)
            .args(args)
            .arg("--listen")
            .arg("stdio://")
            .env_remove("CODEX_CPA_AUTH_DIR")
            .env("CODEX_CPA_AUTH_FILE", root.join(id))
            .env("CODEX_CPA_CREDENTIAL_ID", id)
            .env("CODEX_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        crate::cpa_bridge::record_request(
            id,
            "",
            "cpa.worker.started",
            json!({
            "pid": child.id(), "codexHome": home,
            "configFile": home.join("config.toml"), "credentialFile": root.join(id)}),
        );
        let stdin = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let worker = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            sequence: AtomicU64::new(/*v*/ 1),
            pending: Arc::default(),
            streams: Arc::default(),
        });
        let pending = worker.pending.clone();
        let streams = worker.streams.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(mut message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(id) = message["id"].as_u64() {
                    let target = pending.lock().await.remove(&id);
                    match target {
                        Some(Pending::Reply(reply)) => {
                            let _ = reply.send(message);
                        }
                        Some(Pending::Forward {
                            id,
                            client,
                            request_id,
                        }) => {
                            if message.get("error").is_some()
                                && let Some(request_id) = request_id
                            {
                                streams.lock().await.remove(&request_id);
                            }
                            message["id"] = id;
                            let _ = client.send(message).await;
                        }
                        None => {}
                    }
                } else if let Some(request_id) = message["params"]["requestId"].as_str() {
                    let terminal = message["method"] == "cpa/inference/completed";
                    let client = {
                        let mut streams = streams.lock().await;
                        if terminal {
                            streams.remove(request_id)
                        } else {
                            streams.get(request_id).cloned()
                        }
                    };
                    if let Some(client) = client {
                        let _ = client.send(message).await;
                    }
                }
            }
            let pending = std::mem::take(&mut *pending.lock().await);
            for (_, target) in pending {
                let response = error(Value::Null, 503, "Account runtime stopped");
                match target {
                    Pending::Reply(reply) => {
                        let _ = reply.send(response);
                    }
                    Pending::Forward {
                        id,
                        client,
                        request_id,
                    } => {
                        if let Some(request_id) = request_id {
                            streams.lock().await.remove(&request_id);
                        }
                        let _ = client.send(error(id, 503, "Account runtime stopped")).await;
                    }
                }
            }
            let streams = std::mem::take(&mut *streams.lock().await);
            // Only requests whose start reply already succeeded remain here.
            // A dead worker cannot deliver their atomic completed frame.
            for (request_id, client) in streams {
                let _ = client.send(json!({"method":"cpa/inference/error","params":{
                    "requestId":request_id,"httpStatus":503,"message":"Account runtime stopped","body":null,"headers":null
                }})).await;
            }
        });
        let response = tokio::time::timeout(Duration::from_secs(/*secs*/ 30), worker.rpc("initialize", json!({
            "clientInfo":{"name":"codex-tui","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}
        }))).await??;
        anyhow::ensure!(
            response.get("result").is_some(),
            "child initialization failed: {response}"
        );
        worker.write(json!({"method":"initialized"})).await?;
        Ok(worker)
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Concurrent requests must serialize whole JSON lines into the same child pipe"
    )]
    async fn write(&self, message: Value) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.stdin.lock().await.write_all(&bytes).await?;
        Ok(())
    }

    pub(super) async fn rpc(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.sequence.fetch_add(/*val*/ 1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, Pending::Reply(tx));
        if let Err(error) = self
            .write(json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        rx.await.context("account runtime stopped")
    }

    pub(super) async fn forward(&self, mut message: Value, client: Client) -> anyhow::Result<()> {
        let request_id = message["params"]["requestId"]
            .as_str()
            .context("requestId required")?
            .to_owned();
        let inference = message["method"] == "cpa/inference/start";
        if inference {
            let mut streams = self.streams.lock().await;
            anyhow::ensure!(
                !streams.contains_key(&request_id),
                "Duplicate active requestId"
            );
            streams.insert(request_id.clone(), client.clone());
        }
        let id = self.sequence.fetch_add(/*val*/ 1, Ordering::Relaxed);
        self.pending.lock().await.insert(
            id,
            Pending::Forward {
                id: message["id"].clone(),
                client,
                request_id: inference.then_some(request_id.clone()),
            },
        );
        message["id"] = json!(id);
        if let Err(error) = self.write(message).await {
            self.pending.lock().await.remove(&id);
            if inference {
                self.streams.lock().await.remove(&request_id);
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) async fn owns_request(&self, request_id: &str, client: &Client) -> bool {
        self.streams
            .lock()
            .await
            .get(request_id)
            .is_some_and(|owner| owner.same_channel(client))
    }

    pub(super) async fn disconnect(&self, client: &Client) {
        self.pending
            .lock()
            .await
            .retain(|_, pending| match pending {
                Pending::Forward { client: owner, .. } => !owner.same_channel(client),
                Pending::Reply(_) => true,
            });
        let ids: Vec<_> = self
            .streams
            .lock()
            .await
            .iter()
            .filter(|(_, owner)| owner.same_channel(client))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.streams.lock().await.remove(&id);
            let _ = self
                .write(
                    json!({"id":self.sequence.fetch_add(/*val*/ 1, Ordering::Relaxed),
                "method":"cpa/inference/cancel","params":{"requestId":id}}),
                )
                .await;
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Serialize termination and reaping of the owned child process"
    )]
    pub(super) async fn stop(&self) -> anyhow::Result<()> {
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            // A disabled account must not drain inference or flush network exporters.
            child.kill().await?;
        }
        child.wait().await?;
        Ok(())
    }
}
