use super::*;
use codex_login::ManualLogin;
use codex_login::ServerOptions;

pub(super) struct Login {
    id: String,
    pending: Option<ManualLogin>,
    result: CpaAuthLoginStatusResponse,
}

impl CpaBridge {
    pub(crate) fn login_start(
        &self,
        params: CpaAuthLoginStartParams,
    ) -> BridgeResult<CpaAuthLoginStartResponse> {
        if self.credential_id.as_deref() != Some(params.credential_id.as_str()) {
            return Err(failure(/*status*/ 403, "credentialId mismatch"));
        }
        self.require_enabled()?;
        let mut options = ServerOptions::new(
            self.config.codex_home.to_path_buf(),
            codex_login::oauth_client_id(),
            self.auth.effective_chatgpt_workspaces(),
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
            self.config.auth_route_config(),
        );
        if let Ok(issuer) = std::env::var("CODEX_APP_SERVER_LOGIN_ISSUER")
            && !issuer.is_empty()
        {
            options.issuer = issuer;
        }
        let pending = ManualLogin::start(options)
            .map_err(|_| failure(/*status*/ 500, "Unable to start OAuth"))?;
        let response = CpaAuthLoginStartResponse {
            login_id: uuid::Uuid::now_v7().to_string(),
            auth_url: pending.auth_url.clone(),
            state: pending.state.clone(),
        };
        *self
            .login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Login {
            id: response.login_id.clone(),
            pending: Some(pending),
            result: CpaAuthLoginStatusResponse {
                status: "pending".into(),
                error: None,
            },
        });
        Ok(response)
    }

    pub(crate) fn login_status(&self, id: &str) -> BridgeResult<CpaAuthLoginStatusResponse> {
        self.login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|login| login.id == id)
            .map(|login| login.result.clone())
            .ok_or_else(|| failure(/*status*/ 404, "Unknown loginId"))
    }

    pub(crate) async fn login_callback(
        &self,
        params: CpaAuthLoginCallbackParams,
    ) -> BridgeResult<CpaAuthLoginStatusResponse> {
        self.require_enabled()?;
        let pending = self
            .login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            .filter(|login| login.id == params.login_id)
            .and_then(|login| login.pending.take())
            .ok_or_else(|| failure(/*status*/ 404, "Unknown pending loginId"))?;
        let result = match pending.complete(&params.redirect_url).await {
            Ok(tokens) => self.require_enabled()?.save_tokens(tokens),
            Err(err) => Err(err),
        };
        let result = CpaAuthLoginStatusResponse {
            status: if result.is_ok() { "completed" } else { "error" }.into(),
            error: result.err().map(|_| "OAuth login failed".into()),
        };
        if let Some(login) = self
            .login
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
            && login.id == params.login_id
        {
            login.result = result.clone();
        }
        self.auth.reload().await;
        if result.status == "completed" {
            self.config_manager
                .update_managed_config_loader(self.auth.clone(), &self.config);
            self.config_manager
                .sync_default_client_residency_requirement()
                .await;
        }
        Ok(result)
    }

    pub(crate) async fn reload(&self) -> BridgeResult<CpaCapabilitiesReadResponse> {
        self.auth.reload().await;
        if self.require_enabled().is_err() {
            for token in self
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
            {
                token.cancel();
            }
            *self
                .login
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        self.capabilities()
    }

    pub(super) fn require_enabled(&self) -> BridgeResult<&codex_login::CpaCredentialFile> {
        self.auth
            .cpa_credential_file()
            .filter(|file| file.is_enabled())
            .ok_or_else(|| {
                failure(/*status*/ 403, "Credential is disabled")
            })
    }
}
