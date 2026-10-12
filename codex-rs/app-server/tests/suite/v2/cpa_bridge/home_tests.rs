use super::*;
use pretty_assertions::assert_eq;
use sha2::Digest;

#[tokio::test]
async fn cpa_legacy_home_migrates_to_credential_name_with_state_intact() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let state = home.path().join("state");
    let named_home = state.join("shared.json");
    let legacy_home = state.join(format!("{:x}", sha2::Sha256::digest(b"shared.json")));
    let config = std::fs::read(named_home.join("config.toml"))?;
    let credential = std::fs::read(home.path().join("shared.json"))?;
    std::fs::create_dir(named_home.join("retained-state"))?;
    std::fs::write(named_home.join("retained-state/marker"), b"account state")?;
    std::fs::rename(&named_home, &legacy_home)?;

    let runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":50,"method":"cpa/capabilities/read","params":{}}),
    )
    .await?;
    assert_eq!(
        receive(&mut ws, "id", json!(50)).await?["result"]["protocolVersion"],
        json!(4)
    );
    assert!(!legacy_home.exists());
    assert_eq!(std::fs::read(named_home.join("config.toml"))?, config);
    assert_eq!(
        std::fs::read(named_home.join("retained-state/marker"))?,
        b"account state"
    );
    assert_eq!(std::fs::read(home.path().join("shared.json"))?, credential);
    assert!(!named_home.join("auth.json").exists());
    ws.close(/*msg*/ None).await?;
    drop(runtime);

    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":51,"method":"cpa/auth/login/start","params":{"credentialId":"shared.json"}}),
    )
    .await?;
    let login = receive(&mut ws, "id", json!(51)).await?;
    assert!(login["result"]["loginId"].is_string(), "{login}");
    assert!(!legacy_home.exists());
    assert_eq!(
        std::fs::read(named_home.join("retained-state/marker"))?,
        b"account state"
    );
    Ok(())
}

#[tokio::test]
async fn cpa_existing_credential_named_home_takes_precedence_without_overwriting() -> Result<()> {
    let upstream = MockServer::start().await;
    let home = TempDir::new()?;
    prepare_account(&upstream, &home)?;
    let state = home.path().join("state");
    let named_home = state.join("shared.json");
    let legacy_home = state.join(format!("{:x}", sha2::Sha256::digest(b"shared.json")));
    std::fs::create_dir(&legacy_home)?;
    std::fs::write(named_home.join("marker"), b"current")?;
    std::fs::write(legacy_home.join("marker"), b"legacy")?;
    std::fs::write(legacy_home.join("config.toml"), b"invalid toml [")?;

    let _runtime = launch(&upstream, &home).await?;
    let mut ws = connect(&home).await?;
    send(
        &mut ws,
        json!({"id":52,"method":"cpa/auth/login/start","params":{"credentialId":"shared.json"}}),
    )
    .await?;
    let login = receive(&mut ws, "id", json!(52)).await?;
    assert!(login["result"]["loginId"].is_string(), "{login}");
    assert_eq!(
        (
            std::fs::read(named_home.join("marker"))?,
            std::fs::read(legacy_home.join("marker"))?
        ),
        (b"current".to_vec(), b"legacy".to_vec())
    );
    Ok(())
}
