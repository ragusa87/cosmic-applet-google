use anyhow::{Context, Result, anyhow, bail};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    RefreshToken, Scope, TokenResponse, TokenUrl, basic::BasicClient,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use crate::secrets::Tokens;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";

const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization complete</title><link rel=\"icon\" href=\"data:,\"></head><body style=\"font-family:sans-serif;text-align:center;padding-top:4em\"><h1>You can close this tab</h1><p>The agenda applet has received your authorization.</p></body></html>";

fn http_client() -> Result<oauth2::reqwest::Client> {
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .context("build http client")
}

fn build_client(
    client_id: &str,
    client_secret: &str,
    redirect: &str,
) -> Result<
    BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    let client = BasicClient::new(ClientId::new(client_id.to_owned()))
        .set_client_secret(ClientSecret::new(client_secret.to_owned()))
        .set_auth_uri(AuthUrl::new(AUTH_URL.to_owned())?)
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_owned())?)
        .set_redirect_uri(RedirectUrl::new(redirect.to_owned())?);
    Ok(client)
}

fn expires_at_from(token: &impl TokenResponse) -> u64 {
    let secs = token
        .expires_in()
        .unwrap_or_else(|| Duration::from_hours(1))
        .as_secs();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    now + secs
}

/// Drive the full PKCE + loopback flow. Opens the browser, listens for the
/// redirect, exchanges the code, returns fully-populated `Tokens`.
pub async fn start_oauth_flow(client_id: String, client_secret: String) -> Result<Tokens> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind loopback listener")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let client = build_client(&client_id, &client_secret, &redirect)?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

    let (auth_url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(SCOPE.to_owned()))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .set_pkce_challenge(challenge)
        .url();

    tracing::info!("opening browser for OAuth consent (loopback 127.0.0.1:{port})");
    if let Err(e) = tokio::process::Command::new("xdg-open")
        .arg(auth_url.as_str())
        .status()
        .await
    {
        tracing::warn!(
            error = %e,
            "failed to invoke xdg-open; user must open the URL manually: {}",
            auth_url
        );
    }

    let (code, state) = tokio::time::timeout(Duration::from_mins(5), wait_for_redirect(listener))
        .await
        .map_err(|_| anyhow!("timed out waiting for the OAuth redirect"))??;

    if state != *csrf.secret() {
        bail!("OAuth state mismatch (possible CSRF)");
    }

    let http = http_client()?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&http)
        .await
        .context("exchange authorization code")?;

    let refresh = token
        .refresh_token()
        .ok_or_else(|| anyhow!("Google did not return a refresh_token (re-run with prompt=consent)"))?
        .secret()
        .clone();

    Ok(Tokens {
        client_secret,
        refresh_token: refresh,
        access_token: token.access_token().secret().clone(),
        expires_at_unix: expires_at_from(&token),
    })
}

/// Exchange a refresh token for a new access token. Re-uses the existing
/// `refresh_token` if Google does not return a new one (which is the norm).
pub async fn refresh(
    client_id: &str,
    tokens: &Tokens,
) -> Result<Tokens> {
    let client = build_client(client_id, &tokens.client_secret, "http://127.0.0.1")?;
    let http = http_client()?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(tokens.refresh_token.clone()))
        .request_async(&http)
        .await
        .context("refresh access token")?;

    let new_refresh = token
        .refresh_token()
        .map_or_else(|| tokens.refresh_token.clone(), |r| r.secret().clone());

    Ok(Tokens {
        client_secret: tokens.client_secret.clone(),
        refresh_token: new_refresh,
        access_token: token.access_token().secret().clone(),
        expires_at_unix: expires_at_from(&token),
    })
}

async fn wait_for_redirect(listener: TcpListener) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept().await.context("accept redirect")?;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.context("read redirect request")?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let first_line = request.lines().next().ok_or_else(|| anyhow!("empty request"))?;
    let path = first_line.split_whitespace().nth(1).ok_or_else(|| anyhow!("malformed request line"))?;

    let full = format!("http://localhost{path}");
    let url = Url::parse(&full).context("parse redirect URL")?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        SUCCESS_HTML.len(),
        SUCCESS_HTML,
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    if let Some(e) = error {
        bail!("OAuth provider returned error: {e}");
    }
    Ok((
        code.ok_or_else(|| anyhow!("redirect missing 'code' parameter"))?,
        state.ok_or_else(|| anyhow!("redirect missing 'state' parameter"))?,
    ))
}
