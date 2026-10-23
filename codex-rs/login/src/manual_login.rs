//! Browser OAuth using the official CLI flow with a caller-supplied callback URL.
use crate::ServerOptions;
use crate::TokenData;
use crate::callback_params::LIFE_SCIENCES_OAUTH_STATE_SUFFIX;
use crate::oauth::CallbackParameters;
use crate::oauth::PkceCodes;
use crate::oauth::generate_pkce;
use crate::oauth::generate_state;
use crate::server::build_authorize_url;
use crate::server::ensure_workspace_allowed;
use crate::server::exchange_code_for_tokens;
use crate::token_data::parse_chatgpt_jwt_claims;
use std::io;
use url::Url;

pub struct ManualLogin {
    pub auth_url: String,
    pub state: String,
    redirect_uri: String,
    pkce: PkceCodes,
    options: ServerOptions,
}

impl ManualLogin {
    pub fn start(options: ServerOptions) -> io::Result<Self> {
        let pkce = generate_pkce();
        let state = generate_state();
        let redirect_uri = format!("http://127.0.0.1:{}/auth/callback", options.port);
        let auth_url = build_authorize_url(
            &options.issuer,
            &options.client_id,
            &redirect_uri,
            &pkce,
            &state,
            options.forced_chatgpt_workspace_id.as_deref(),
        )?;
        Ok(Self {
            auth_url,
            state,
            redirect_uri,
            pkce,
            options,
        })
    }

    /// Parses the registered callback locally. No request is sent to the supplied URL.
    pub async fn complete(self, redirect_url: &str) -> io::Result<TokenData> {
        let url = Url::parse(redirect_url).map_err(|_| io::Error::other("Invalid callback URL"))?;
        let mut base = url.clone();
        base.set_query(/*query*/ None);
        if base.as_str() != self.redirect_uri {
            return Err(io::Error::other("Callback URL mismatch"));
        }
        let mut params = CallbackParameters::from_url(&url);
        if let Some(state) = params.state.as_mut()
            && state.strip_suffix(LIFE_SCIENCES_OAUTH_STATE_SUFFIX) == Some(&self.state)
        {
            state.truncate(self.state.len());
        }
        let code = params
            .validate(&self.state)
            .map_err(|_| io::Error::other("Invalid OAuth callback"))?;
        let (tokens, _) = exchange_code_for_tokens(
            &self.options.issuer,
            &self.options.client_id,
            &self.redirect_uri,
            &self.pkce,
            code,
            &self.options.auth_route_config,
        )
        .await
        .map_err(|_| io::Error::other("OAuth code exchange failed"))?;
        ensure_workspace_allowed(
            self.options.forced_chatgpt_workspace_id.as_deref(),
            &tokens.id_token,
        )
        .map_err(|_| io::Error::other("OAuth workspace mismatch"))?;
        let id_token = parse_chatgpt_jwt_claims(&tokens.id_token)
            .map_err(|_| io::Error::other("Invalid ID token"))?;
        Ok(TokenData {
            account_id: id_token.chatgpt_account_id.clone(),
            id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            ..Default::default()
        })
    }
}
