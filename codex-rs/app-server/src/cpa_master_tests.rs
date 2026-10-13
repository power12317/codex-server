use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn cpa_business_precedes_queued_diagnostics_without_losing_the_tail() {
    let (client, mut messages) = Client::channel();
    for sequence in 0..100 {
        client
            .send(json!({"method":"cpa/inference/upstream","params":{"sequence":sequence}}))
            .await
            .unwrap();
    }
    client.diagnostics_completed("r").await;
    client
        .send(json!({"method":"cpa/inference/event"}))
        .await
        .unwrap();
    client
        .send(json!({"method":"cpa/inference/completed"}))
        .await
        .unwrap();
    assert_eq!(
        messages.recv().await.unwrap()["method"],
        "cpa/inference/event"
    );
    assert_eq!(
        messages.recv().await.unwrap()["method"],
        "cpa/inference/completed"
    );
    for sequence in 0..100 {
        assert_eq!(
            messages.recv().await.unwrap()["params"]["sequence"],
            sequence
        );
    }
    assert_eq!(
        messages.recv().await.unwrap()["method"],
        "cpa/inference/diagnostics/completed"
    );
    drop(client);
    assert!(messages.recv().await.is_none());
}

#[test]
fn scan_uses_relative_credential_ids_and_effective_flags() -> anyhow::Result<()> {
    let directory = tempfile::TempDir::new()?;
    let root = directory.path();
    std::fs::create_dir(root.join("nested"))?;
    for (name, value) in [
        (
            "Original Account.JSON",
            json!({"type":" Codex ","codex_cli":{"enabled":true}}),
        ),
        (
            "nested/account.json",
            json!({"type":"codex","codex_cli":{"enabled":true},"access_token":"updated"}),
        ),
        (
            "off.json",
            json!({"type":"codex","codex_cli":{"enabled":false}}),
        ),
        (
            "disabled.json",
            json!({"type":"codex","disabled":true,"codex_cli":{"enabled":true}}),
        ),
        (
            "other.json",
            json!({"type":"other","codex_cli":{"enabled":true}}),
        ),
    ] {
        std::fs::write(root.join(name), serde_json::to_vec(&value)?)?;
    }
    assert_eq!(
        scan(root)?,
        HashMap::from([
            (
                "Original Account.JSON".into(),
                root.join("Original Account.JSON")
            ),
            (
                "nested/account.json".into(),
                root.join("nested/account.json")
            ),
        ])
    );
    Ok(())
}

#[tokio::test]
async fn local_bridge_accepts_messages_larger_than_default_limits() -> anyhow::Result<()> {
    use tokio_tungstenite::tungstenite::Message as ClientMessage;

    let directory = tempfile::TempDir::new()?;
    let master = Arc::new(Master {
        shutdown: CancellationToken::new(),
        root: directory.path().to_owned(),
        state: directory.path().to_owned(),
        workers: Mutex::default(),
        logins: Mutex::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let app = router(master);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let result: anyhow::Result<()> = async {
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{address}/cpa/v1/ws")).await?;
        let padding = "x".repeat((64 << 20) + 1);
        socket
            .send(ClientMessage::Text(
                json!({"id":1,"method":"initialize","params":{
                    "clientInfo":{"name":"codex_app_server_daemon","version":env!("CARGO_PKG_VERSION")},
                    "padding":padding
                }})
                    .to_string()
                    .into(),
            ))
            .await?;
        let response = socket
            .next()
            .await
            .context("large RPC response missing")??;
        let response: Value = serde_json::from_str(response.to_text()?)?;
        assert_eq!(response["id"], json!(1));
        assert!(response.get("result").is_some());
        assert_eq!(
            response["result"]["userAgent"],
            json!(get_codex_user_agent())
        );
        assert!(response["result"].get("serverInfo").is_none());
        socket
            .send(ClientMessage::Text(
                json!({"id":2,"method":"cpa/capabilities/read","params":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let response = socket
            .next()
            .await
            .context("follow-up RPC response missing")??;
        let response: Value = serde_json::from_str(response.to_text()?)?;
        assert_eq!(response["id"], json!(2));
        assert_eq!(response["result"]["protocolVersion"], json!(5));
        socket.close(None).await?;
        Ok(())
    }
    .await;
    let _ = shutdown_tx.send(());
    server.await??;
    result
}
