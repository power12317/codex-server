//! CPA's flat credential file is the only token store for a managed worker.
use super::*;
use serde_json::Value;
use serde_json::json;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;

/// Keep the same stable sidecar as CPA locked through the final credential rename.
struct CredentialLock(File);

impl CredentialLock {
    fn acquire(path: &std::path::Path) -> io::Result<Self> {
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock_path)?;
        #[cfg(test)]
        tests::before_lock(&file);
        // std uses flock(LOCK_EX) on Unix and LockFileEx on Windows. The Windows
        // range includes byte zero, overlapping CPA's one-byte exclusive lock.
        file.lock()?;
        Ok(Self(file))
    }
}

impl Drop for CredentialLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
        // Never unlink the sidecar: another process may already be waiting on it.
    }
}

fn canonical_credential_path(path: &std::path::Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if let Ok(resolved) = std::fs::canonicalize(&absolute) {
        return Ok(resolved);
    }
    if let Some(parent) = absolute.parent()
        && let Some(name) = absolute.file_name()
        && let Ok(parent) = std::fs::canonicalize(parent)
    {
        return Ok(parent.join(name));
    }
    Ok(absolute)
}

#[derive(Clone, Debug)]
pub struct CpaCredentialFile {
    pub path: PathBuf,
    pub credential_id: String,
}

impl CpaCredentialFile {
    pub fn read(&self) -> io::Result<Value> {
        serde_json::from_slice(&std::fs::read(&self.path)?).map_err(io::Error::other)
    }

    pub fn is_enabled(&self) -> bool {
        self.read().is_ok_and(|value| enabled(&value))
    }

    pub fn save_tokens(&self, tokens: TokenData) -> io::Result<()> {
        self.save(&auth_json(tokens, Some(Utc::now())))
    }

    pub(super) async fn auth(&self, route: &AuthRouteConfig) -> io::Result<Option<CodexAuth>> {
        let Some(auth_dot_json) = self.load()? else {
            return Ok(None);
        };
        let client = create_default_auth_client(&refresh_token_endpoint(None), route)?;
        Ok(Some(CodexAuth::Chatgpt(ChatgptAuth {
            state: ChatgptAuthState {
                auth_dot_json: Arc::new(Mutex::new(Some(auth_dot_json))),
                client,
            },
            storage: Arc::new(self.clone()),
        })))
    }
}

impl AuthStorageBackend for CpaCredentialFile {
    fn load(&self) -> io::Result<Option<AuthDotJson>> {
        let value = self.read()?;
        if !enabled(&value)
            || value["access_token"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        {
            return Ok(None);
        }
        let tokens = serde_json::from_value(json!({
            "id_token": value["id_token"], "access_token": value["access_token"],
            "refresh_token": value["refresh_token"], "account_id": value["account_id"]
        }))?;
        let last_refresh = value["last_refresh"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));
        Ok(Some(auth_json(tokens, last_refresh)))
    }

    fn save(&self, auth: &AuthDotJson) -> io::Result<()> {
        let path = canonical_credential_path(&self.path)?;
        let _lock = CredentialLock::acquire(&path)?;
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&path)?).map_err(io::Error::other)?;
        if !enabled(&value) {
            return Err(io::Error::other("CPA credential is disabled"));
        }
        let tokens = auth
            .tokens
            .as_ref()
            .ok_or_else(|| io::Error::other("Missing tokens"))?;
        value["type"] = json!("codex");
        value["id_token"] = json!(tokens.id_token.raw_jwt);
        value["access_token"] = json!(tokens.access_token);
        value["refresh_token"] = json!(tokens.refresh_token);
        value["account_id"] = json!(tokens.account_id);
        value["email"] = json!(tokens.id_token.email);
        value["last_refresh"] = json!(auth.last_refresh);
        if let Ok(Some(expired)) = parse_jwt_expiration(&tokens.access_token) {
            value["expired"] = json!(expired);
        }
        let temporary = path.with_file_name(format!(".cpa-{}.tmp", crate::oauth::generate_state()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec(&value)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    fn delete(&self) -> io::Result<bool> {
        Err(io::Error::other("CPA manages credential removal"))
    }
}

#[cfg(test)]
#[path = "cpa_tests.rs"]
mod tests;

fn enabled(value: &Value) -> bool {
    value["type"]
        .as_str()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("codex"))
        && value["codex_cli"]["enabled"] == true
        && value["disabled"] != true
}

fn auth_json(tokens: TokenData, last_refresh: Option<chrono::DateTime<Utc>>) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::Chatgpt),
        openai_api_key: None,
        tokens: Some(tokens),
        last_refresh,
        agent_identity: None,
        personal_access_token: None,
        bedrock_api_key: None,
        bedrock_access_keys: None,
    }
}

impl AuthManager {
    /// Uses CPA's configured file instead of CODEX_HOME/auth.json or environment credentials.
    pub async fn shared_with_cpa_credentials(
        config: &impl AuthManagerConfig,
        file: CpaCredentialFile,
    ) -> Arc<Self> {
        Arc::new(
            Self::new_from_auth_config(
                auth_config_from(config),
                /*enable_codex_api_key_env*/ false,
                Some(file),
            )
            .await,
        )
    }

    pub fn cpa_credential_file(&self) -> Option<&CpaCredentialFile> {
        self.cpa_file.as_ref()
    }
}
