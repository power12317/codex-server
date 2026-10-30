use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

// Independent test decoder covers the exact CBOR subset emitted on the wire.
#[derive(Debug, PartialEq)]
enum Cbor {
    Uint(u64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Float(f64),
}
fn decode(input: &mut &[u8]) -> Cbor {
    let byte = input[0];
    *input = &input[1..];
    let major = byte >> 5;
    let count = match byte & 31 {
        n @ 0..=23 => n as u64,
        n @ 24..=27 => {
            let width = 1usize << (n - 24);
            let mut value = 0u64;
            for b in &input[..width] {
                value = (value << 8) | u64::from(*b);
            }
            *input = &input[width..];
            value
        }
        _ => panic!("unexpected CBOR"),
    };
    if byte == 0xfb {
        return Cbor::Float(f64::from_bits(count));
    }
    match major {
        0 => Cbor::Uint(count),
        2 | 3 => {
            let bytes = input[..count as usize].to_vec();
            *input = &input[count as usize..];
            if major == 2 {
                Cbor::Bytes(bytes)
            } else {
                Cbor::Text(String::from_utf8(bytes).unwrap())
            }
        }
        4 => Cbor::Array((0..count).map(|_| decode(input)).collect()),
        5 => Cbor::Map((0..count).map(|_| (decode(input), decode(input))).collect()),
        _ => panic!("unexpected CBOR major"),
    }
}
fn field<'a>(value: &'a Cbor, key: Cbor) -> &'a Cbor {
    let Cbor::Map(map) = value else {
        panic!("map expected")
    };
    &map.iter().find(|(k, _)| *k == key).unwrap().1
}
fn unpack(identity: &DesktopIdentity, time: &str) -> (Cbor, Cbor) {
    let raw = identity.attestation_at(time.parse().unwrap());
    let value: Value = serde_json::from_str(raw.to_str().unwrap()).unwrap();
    assert_eq!(value["v"], 1);
    assert_eq!(value["s"], 0);
    assert_eq!(value.as_object().unwrap().len(), 3);
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value["t"].as_str().unwrap().strip_prefix("v1.").unwrap())
        .unwrap();
    let mut input = bytes.as_slice();
    let inner = decode(&mut input);
    assert!(input.is_empty());
    let Cbor::Bytes(signals) = field(&inner, Cbor::Text("f".into())) else {
        panic!("binary fingerprint")
    };
    let mut input = signals.as_slice();
    let signals = decode(&mut input);
    assert!(input.is_empty());
    (inner, signals)
}
fn fixture(system: &str) -> DesktopIdentity {
    let mut identity = DesktopIdentity::from_credential(
        &json!({"account_id":"synthetic-account","codex_client_system":system}),
    );
    identity.timezone = TimeZone::get("Asia/Tokyo").unwrap();
    identity.timezone_name = "Asia/Tokyo".into();
    identity
}
#[test]
fn cpa_desktop_platform_status_and_host_engine_versions_are_distinct() {
    for (system, error, platform) in [
        ("windows", 1, "Windows 10.0.19044; x86_64"),
        ("mac", 2, "Mac OS 15.3.1; x86_64"),
    ] {
        let identity = fixture(system);
        assert_eq!(
            identity.user_agent(),
            format!(
                "Codex Desktop/{} ({platform}) dumb (Codex Desktop; 26.1023.51421)",
                env!("CARGO_PKG_VERSION")
            )
        );
        let (inner, signals) = unpack(&identity, "2026-10-30T12:00:00Z");
        assert_eq!(
            field(&inner, Cbor::Text("error_code".into())),
            &Cbor::Uint(error)
        );
        assert_eq!(
            field(&inner, Cbor::Text("bundle_id".into())),
            &Cbor::Text("com.openai.codex".into())
        );
        let Cbor::Map(map) = inner else {
            unreachable!()
        };
        assert_eq!(map.len(), 3);
        assert_eq!(field(&signals, Cbor::Uint(0)), &Cbor::Uint(1));
        assert_eq!(
            field(&signals, Cbor::Uint(1)),
            &Cbor::Array(vec![Cbor::Text("ja-JP".into())])
        );
        assert_eq!(
            field(&signals, Cbor::Uint(3)),
            &Cbor::Text("Asia/Tokyo".into())
        );
        assert!(matches!(
            field(&signals, Cbor::Uint(4)),
            Cbor::Uint(1000..=7000)
        ));
    }
}
#[test]
fn cpa_desktop_daily_session_uses_service_zone_and_stable_device() {
    let identity = fixture("mac");
    let (_, before) = unpack(&identity, "2026-10-30T14:59:00Z");
    let (_, same) = unpack(&identity, "2026-10-30T00:00:00Z");
    let (_, after) = unpack(&identity, "2026-10-30T15:00:00Z");
    assert_eq!(before, same);
    assert_ne!(field(&before, Cbor::Uint(6)), field(&after, Cbor::Uint(6)));
    assert_eq!(field(&before, Cbor::Uint(4)), field(&after, Cbor::Uint(4)));
    assert_ne!(
        unpack(&fixture("windows"), "2026-10-30T00:00:00Z").1,
        before
    );
}
#[test]
fn cpa_desktop_credential_rename_and_refresh_do_not_change_profile() {
    let first = json!({"account_id":"account","email":"a@example.test","codex_client_system":"windows","access_token":"fake-before","credential_id":"before.json"});
    let mut second = first.clone();
    second["access_token"] = json!("fake-after");
    second["credential_id"] = json!("after.json");
    second["email"] = json!("renamed@example.test");
    let a = DesktopIdentity::from_credential(&first);
    let b = DesktopIdentity::from_credential(&second);
    assert_eq!(a.seed, b.seed);
    assert_eq!(
        a.attestation_at("2026-10-30T00:00:00Z".parse().unwrap()),
        b.attestation_at("2026-10-30T00:00:00Z".parse().unwrap())
    );
    second["account_id"] = json!("other-account");
    assert_ne!(a.seed, DesktopIdentity::from_credential(&second).seed);
    for metadata in [
        json!({}),
        json!({"codex_client_system":"mac"}),
        json!({"codex_client_system":"legacy"}),
    ] {
        assert_eq!(
            DesktopIdentity::from_credential(&metadata).system,
            DesktopSystem::Mac
        );
    }
}
#[test]
fn cpa_desktop_does_not_install_in_an_unmanaged_process() {
    assert!(worker_identity().is_none());
    let headers = crate::default_client::default_headers();
    assert!(!headers.contains_key("x-oai-attestation"));
    let configured: codex_model_provider_info::ModelProviderInfo = serde_json::from_value(json!({
        "http_headers":{"User-Agent":"unmanaged-agent","OrIgInAtOr":"unmanaged-originator",
            "VERSION":"legacy-version","X-OAI-ATTESTATION":"unmanaged-proof",
            "X-Custom":"preserved"}
    }))
    .unwrap();
    let provider = configured.to_api_provider(None).unwrap();
    for (name, value) in [
        ("user-agent", "unmanaged-agent"),
        ("originator", "unmanaged-originator"),
        ("version", "legacy-version"),
        ("x-oai-attestation", "unmanaged-proof"),
        ("x-custom", "preserved"),
    ] {
        assert_eq!(provider.headers[name], value);
    }
}

#[test]
#[ignore = "isolated helper for managed process defaults"]
fn cpa_desktop_managed_defaults_child() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.json");
    let system = std::env::var("CODEX_DESKTOP_TEST_SYSTEM").unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"account_id":"synthetic","codex_client_system":system}))
            .unwrap(),
    )
    .unwrap();
    install_for_worker(&crate::CpaCredentialFile {
        path,
        credential_id: "synthetic.json".into(),
    })
    .unwrap();
    let _ = crate::default_client::set_default_originator("codex-tui".into());
    *crate::default_client::USER_AGENT_SUFFIX.lock().unwrap() = Some("caller-cli; 9".into());
    let headers = crate::default_client::default_headers();
    assert_eq!(headers["originator"], "Codex Desktop");
    assert_eq!(
        headers["user-agent"],
        worker_identity().unwrap().user_agent()
    );
    assert!(
        !headers.contains_key("x-oai-attestation"),
        "non-inference/auth default headers must not contain proof"
    );
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let upstream=wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::any()).respond_with(wiremock::ResponseTemplate::new(200)).mount(&upstream).await;
        let client=crate::default_client::create_client();
        for endpoint in ["/models","/usage"] {
            assert!(client.get(format!("{}{endpoint}",upstream.uri())).send().await.unwrap().status().is_success());
        }
        assert!(client.post(format!("{}/oauth/token",upstream.uri())).body("synthetic").send().await.unwrap().status().is_success());
        let received=upstream.received_requests().await.unwrap();assert_eq!(received.len(),3);
        let mut evidence=Vec::new();
        for request in received {
            assert_eq!(request.headers["originator"],"Codex Desktop");
            assert_eq!(request.headers["user-agent"],worker_identity().unwrap().user_agent());
            assert!(!request.headers.contains_key("x-oai-attestation"));
            evidence.push(json!({"method":request.method.as_str(),"path":request.url.path(),"userAgent":request.headers["user-agent"].to_str().unwrap(),"originator":"Codex Desktop","hasAttestation":false}));
        }
        if let Some(dir)=std::env::var_os("CODEX_DESKTOP_EVIDENCE_DIR") {
            let dir=std::path::PathBuf::from(dir);std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{system}-non-inference.json")),serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
        }
    });
    assert!(
        install_for_worker(&crate::CpaCredentialFile {
            path: dir.path().join("missing"),
            credential_id: "missing".into()
        })
        .is_err()
    );
}
#[test]
fn cpa_desktop_managed_defaults_are_process_isolated() {
    for system in ["windows", "mac"] {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .env("CODEX_DESKTOP_TEST_SYSTEM", system)
            .args([
                "--exact",
                "cpa_desktop::tests::cpa_desktop_managed_defaults_child",
                "--ignored",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert!(worker_identity().is_none());
}

#[test]
fn cpa_desktop_cbor_matches_official_pure_codec_bytes() {
    let golden: Value = serde_json::from_str(include_str!("cpa_desktop_golden.json")).unwrap();
    for system in ["windows", "mac"] {
        let identity = fixture(system);
        let raw = identity.attestation_at(golden["time"].as_str().unwrap().parse().unwrap());
        assert_eq!(
            raw.to_str().unwrap(),
            golden["envelopes"][system].as_str().unwrap()
        );
    }
}
