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
        source_request_id: Option<String>,
    },
}

#[derive(Clone)]
struct StreamTarget {
    client: Client,
    source_request_id: String,
    business_done: bool,
    diagnostics_done: bool,
}

// Keep the route alive until both independent lifetimes have ended. Either
// terminal can arrive first, including diagnostics before a start RPC error.
fn route(
    streams: &mut HashMap<String, StreamTarget>,
    request_id: &str,
    business_done: bool,
    diagnostics_done: bool,
) -> Option<StreamTarget> {
    let target = streams.get_mut(request_id)?;
    target.business_done |= business_done;
    target.diagnostics_done |= diagnostics_done;
    let target = target.clone();
    if target.business_done && target.diagnostics_done {
        streams.remove(request_id);
    }
    Some(target)
}

impl StreamTarget {
    async fn send(&self, mut message: Value) {
        message["params"]["requestId"] = json!(self.source_request_id);
        let _ = self.client.send(message).await;
    }
}

fn register_stream(
    streams: &mut HashMap<String, StreamTarget>,
    source_request_id: &str,
    client: &Client,
    rpc_sequence: u64,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !source_request_id.is_empty()
            && source_request_id.len() <= 256
            && !source_request_id.chars().any(char::is_control),
        "Invalid request identity"
    );
    anyhow::ensure!(
        !streams
            .values()
            .any(|target| target.source_request_id == source_request_id
                && target.client.same_channel(client)),
        "Duplicate active requestId on this connection"
    );
    let internal_id = format!("cpa-rpc-{rpc_sequence}");
    streams.insert(
        internal_id.clone(),
        StreamTarget {
            client: client.clone(),
            source_request_id: source_request_id.into(),
            business_done: false,
            diagnostics_done: false,
        },
    );
    Ok(internal_id)
}

pub(super) struct Worker {
    credential_id: String,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    sequence: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Pending>>>,
    streams: Arc<Mutex<HashMap<String, StreamTarget>>>,
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
        let details = json!({
            "pid": child.id(), "codexHome": home,
            "configFile": home.join("config.toml"), "credentialFile": root.join(id)});
        let credential_id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            crate::cpa_bridge::record_request(&credential_id, "", "cpa.worker.started", details);
        });
        let stdin = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let worker = Arc::new(Self {
            credential_id: id.to_owned(),
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
                            source_request_id,
                        }) => {
                            if message.get("error").is_some()
                                && let Some(request_id) = request_id
                            {
                                route(&mut *streams.lock().await, &request_id, true, false);
                            }
                            message["id"] = id;
                            if let Some(source) = source_request_id
                                && let Some(result) =
                                    message.get_mut("result").and_then(Value::as_object_mut)
                                && result.contains_key("requestId")
                            {
                                result.insert("requestId".into(), json!(source));
                            }
                            let _ = client.send(message).await;
                        }
                        None => {}
                    }
                } else if let Some(request_id) = message["params"]["requestId"].as_str() {
                    let terminal = matches!(
                        message["method"].as_str(),
                        Some("cpa/inference/completed" | "cpa/inference/error")
                    );
                    let diagnostics_done =
                        message["method"] == "cpa/inference/diagnostics/completed";
                    let client = route(
                        &mut *streams.lock().await,
                        request_id,
                        terminal,
                        diagnostics_done,
                    );
                    if let Some(target) = client {
                        target.send(message).await;
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
                        source_request_id: _,
                    } => {
                        if let Some(request_id) = request_id {
                            route(&mut *streams.lock().await, &request_id, true, false);
                        }
                        let _ = client.send(error(id, 503, "Account runtime stopped")).await;
                    }
                }
            }
            let streams = std::mem::take(&mut *streams.lock().await);
            for (_, target) in streams {
                let request_id = &target.source_request_id;
                if !target.business_done {
                    let _ = target.client.send(json!({"method":"cpa/inference/error","params":{
                        "requestId":request_id,"httpStatus":503,"message":"Account runtime stopped","body":null,"headers":null
                    }})).await;
                }
                if !target.diagnostics_done {
                    target.client.diagnostics_completed(request_id).await;
                }
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
        let source_request_id = message["params"]["requestId"]
            .as_str()
            .context("requestId required")?
            .to_owned();
        let inference = message["method"] == "cpa/inference/start";
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        let request_id = if inference {
            register_stream(
                &mut *self.streams.lock().await,
                &source_request_id,
                &client,
                id,
            )?
        } else {
            let internal = self
                .streams
                .lock()
                .await
                .iter()
                .find(|(_, target)| {
                    !target.business_done
                        && target.source_request_id == source_request_id
                        && target.client.same_channel(&client)
                })
                .map(|(id, _)| id.clone());
            let Some(internal) = internal else {
                client.send(json!({"id":message["id"],"result":{}})).await?;
                return Ok(());
            };
            internal
        };
        if inference {
            let credential_id = self.credential_id.clone();
            let source = source_request_id.clone();
            let details = json!({"workerRequestId": request_id, "rpcId": message["id"]});
            tokio::task::spawn_blocking(move || {
                crate::cpa_bridge::record_request(
                    &credential_id,
                    &source,
                    "cpa.master.request",
                    details,
                );
            });
        }
        self.pending.lock().await.insert(
            id,
            Pending::Forward {
                id: message["id"].clone(),
                client,
                request_id: inference.then_some(request_id.clone()),
                source_request_id: inference.then_some(source_request_id),
            },
        );
        message["id"] = json!(id);
        message["params"]["requestId"] = json!(request_id);
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
        self.streams.lock().await.values().any(|target| {
            target.source_request_id == request_id && target.client.same_channel(client)
        })
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
            .filter(|(_, owner)| owner.client.same_channel(client))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cpa_same_request_new_connection_does_not_wait_for_previous_diagnostics() {
        let (first, mut first_messages) = Client::channel();
        let (second, mut second_messages) = Client::channel();
        let mut streams = HashMap::new();
        let first_internal = register_stream(&mut streams, "source-request", &first, 1).unwrap();
        route(&mut streams, &first_internal, true, false)
            .expect("first business ended, diagnostics still open");
        let second_internal = register_stream(&mut streams, "source-request", &second, 2).unwrap();
        assert_ne!(first_internal, second_internal);
        assert_eq!(streams.len(), 2);
        route(&mut streams, &first_internal, false, false).unwrap().send(json!({"method":"cpa/inference/upstream","params":{"requestId":first_internal,"body":"first log"}})).await;
        route(&mut streams, &second_internal, false, false).unwrap().send(json!({"method":"cpa/inference/event","params":{"requestId":second_internal,"bodyBase64":"c2Vjb25k"}})).await;
        let event = second_messages.recv().await.unwrap();
        assert_eq!(event["params"]["requestId"], "source-request");
        assert_eq!(event["method"], "cpa/inference/event");
        let log = first_messages.recv().await.unwrap();
        assert_eq!(log["params"]["requestId"], "source-request");
        assert_eq!(log["params"]["body"], "first log");
        route(&mut streams, &first_internal, false, true)
            .unwrap()
            .send(json!({"method":"cpa/inference/diagnostics/completed","params":{}}))
            .await;
        assert!(streams.contains_key(&second_internal));
        assert_eq!(
            first_messages.recv().await.unwrap()["method"],
            "cpa/inference/diagnostics/completed"
        );
        assert!(second_messages.business.try_recv().is_err());
        assert!(second_messages.diagnostics.try_recv().is_err());
    }

    #[test]
    fn cpa_stream_route_waits_for_both_terminals_in_either_order() {
        for diagnostic_first in [true, false] {
            let (client, _messages) = Client::channel();
            let mut streams = HashMap::from([(
                "r".into(),
                StreamTarget {
                    client,
                    source_request_id: "r".into(),
                    business_done: false,
                    diagnostics_done: false,
                },
            )]);
            assert!(route(&mut streams, "r", !diagnostic_first, diagnostic_first).is_some());
            assert_eq!(streams.len(), 1);
            assert!(
                route(&mut streams, "r", false, false).is_some(),
                "trailing records still route"
            );
            assert!(route(&mut streams, "r", diagnostic_first, !diagnostic_first).is_some());
            assert!(streams.is_empty());
        }
    }
}
