use crate::{
    config::{AuthConfig, Config, Paths, Profile, valid_id},
    events::UiEvent,
    provider,
    session::{atomic_write, hash, lock_file, private_dir},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const RESOURCE: &str = "https://api.openai.com/v1";
const INFERENCE_SCOPE: &str = "chatgpt.tokens.use.direct";

// Credential-bearing types deliberately do not implement Debug.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Credential {
    ApiKey { key: String },
    Chatgpt { session: OAuthSession },
}
#[derive(Serialize, Deserialize, Clone)]
struct OAuthSession {
    issuer: String,
    subject: String,
    email: Option<String>,
    client_id: String,
    ext_agent_host_id: String,
    id_token: String,
    access_token: String,
    refresh_token: String,
    scopes: Vec<String>,
    expires_at: i64,
}
#[derive(Serialize, Deserialize)]
struct Registration {
    client_id: String,
    subject: Option<String>,
}

fn auth_dir(paths: &Paths) -> Result<PathBuf> {
    let dir = paths.config.join("auth");
    private_dir(&paths.config)?;
    private_dir(&dir)?;
    Ok(dir)
}
fn credential_path(paths: &Paths, id: &str) -> Result<PathBuf> {
    ensure!(valid_id(id), "Invalid credential reference");
    Ok(auth_dir(paths)?.join(format!("{id}.json")))
}
fn load(paths: &Paths, id: &str) -> Result<Credential> {
    let path = credential_path(paths, id)?;
    let metadata = fs::metadata(&path).context("Credentials missing; use /connect")?;
    ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "Credential permissions must be owner-only (0600)"
    );
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|_| anyhow::anyhow!("Invalid credential record; use /connect"))
}
fn save(paths: &Paths, id: &str, credential: &Credential) -> Result<()> {
    atomic_write(
        &credential_path(paths, id)?,
        &serde_json::to_vec(credential)?,
        None,
    )
}
pub fn save_key(paths: &Paths, id: &str, key: &str) -> Result<()> {
    ensure!(
        !key.trim().is_empty() && !key.contains(['\n', '\r']),
        "API key must be nonempty and single-line"
    );
    save(paths, id, &Credential::ApiKey { key: key.into() })
}
pub fn disconnect(paths: &Paths, profile: &Profile) -> Result<()> {
    let id = match &profile.auth {
        AuthConfig::Saved { credential } | AuthConfig::Chatgpt { credential } => credential,
        _ => return Ok(()),
    };
    let path = credential_path(paths, id)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    let registration = auth_dir(paths)?.join(format!("{id}.registration.json"));
    if registration.exists() {
        fs::remove_file(registration)?;
    }
    Ok(())
}

pub async fn resolve(
    paths: &Paths,
    profile: &Profile,
    config: &Config,
    cancel: &CancellationToken,
) -> Result<Option<String>> {
    resolve_using(paths, profile, config, cancel, TOKEN).await
}

async fn resolve_using(
    paths: &Paths,
    profile: &Profile,
    config: &Config,
    cancel: &CancellationToken,
    token_endpoint: &str,
) -> Result<Option<String>> {
    match &profile.auth {
        AuthConfig::None => Ok(None),
        AuthConfig::Environment { variable } => {
            let key = std::env::var(variable)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .with_context(|| {
                    format!("Environment variable {variable} is missing; configure this profile")
                })?;
            Ok(Some(key))
        }
        AuthConfig::Saved { credential } => match load(paths, credential)? {
            Credential::ApiKey { key } => Ok(Some(key)),
            _ => bail!("Credential type does not match API-key auth"),
        },
        AuthConfig::Chatgpt { credential } => {
            let Credential::Chatgpt { session } = load(paths, credential)? else {
                bail!("Credential type does not match ChatGPT auth")
            };
            ensure_scope(&session.scopes)?;
            if session.expires_at > Utc::now().timestamp() + 60 {
                return Ok(Some(session.access_token));
            }
            let lock_path = auth_dir(paths)?.join(format!(
                "refresh-{}.lock",
                hash(
                    format!(
                        "{}:{}:{}",
                        session.issuer, session.subject, session.client_id
                    )
                    .as_bytes()
                )
            ));
            let acquire = tokio::task::spawn_blocking(move || lock_file(&lock_path, true));
            let _lock = tokio::select! {_=cancel.cancelled()=>bail!("Token refresh canceled"),result=acquire=>result??};
            // Another process may already have refreshed this rotating token set.
            let Credential::Chatgpt { session: latest } = load(paths, credential)? else {
                bail!("Connection changed during refresh")
            };
            if latest.expires_at > Utc::now().timestamp() + 60 {
                return Ok(Some(latest.access_token));
            }
            let refreshed = tokio::select! {_=cancel.cancelled()=>bail!("Token refresh canceled"),result=refresh(paths,config,credential,latest,token_endpoint)=>result?};
            Ok(Some(refreshed))
        }
    }
}

fn ensure_scope(scopes: &[String]) -> Result<()> {
    ensure!(
        scopes.iter().any(|s| s == INFERENCE_SCOPE),
        "ChatGPT identity is signed in but plan inference permission is missing; reconnect and grant plan access"
    );
    Ok(())
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
    expires_in: u64,
    token_type: String,
}
async fn token_request(
    client: &reqwest::Client,
    endpoint: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse> {
    let response = client
        .post(endpoint)
        .form(form)
        .send()
        .await
        .context("OAuth token exchange could not reach the server")?;
    ensure!(
        response.status().is_success(),
        "OAuth token exchange/refresh was rejected (HTTP {}); reconnect. No credential was replaced.",
        response.status()
    );
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|_| anyhow::anyhow!("Invalid OAuth token response"))?;
    ensure!(
        token.token_type.eq_ignore_ascii_case("bearer")
            && !token.access_token.is_empty()
            && token.expires_in > 0,
        "Invalid OAuth credential type or lifetime"
    );
    Ok(token)
}

async fn refresh(
    paths: &Paths,
    config: &Config,
    id: &str,
    previous: OAuthSession,
    endpoint: &str,
) -> Result<String> {
    let client = provider::client(config)?;
    let response = token_request(
        &client,
        endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &previous.client_id),
            ("refresh_token", &previous.refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let mut next = previous.clone();
    if let Some(id_token) = response.id_token {
        let identity = validate_remote_identity(
            &client,
            &id_token,
            &previous.client_id,
            None,
            Some(&previous.subject),
        )
        .await?;
        next.email = identity.email;
        next.id_token = id_token;
    }
    if let Some(scope) = response.scope {
        next.scopes = scope.split_whitespace().map(str::to_owned).collect();
    }
    ensure_scope(&next.scopes)?;
    next.access_token = response.access_token;
    if let Some(refresh_token) = response.refresh_token {
        next.refresh_token = refresh_token;
    }
    next.expires_at = Utc::now()
        .timestamp()
        .checked_add(i64::try_from(response.expires_in)?)
        .context("Invalid token expiry")?;
    let access = next.access_token.clone();
    save(paths, id, &Credential::Chatgpt { session: next })?;
    Ok(access)
}

#[derive(Deserialize)]
pub struct Identity {
    pub sub: String,
    pub email: Option<String>,
    nonce: Option<String>,
}
#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

async fn validate_remote_identity(
    client: &reqwest::Client,
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    subject: Option<&str>,
) -> Result<Identity> {
    let response = client
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .send()
        .await?;
    ensure!(response.status().is_success(), "OIDC discovery failed");
    let discovery: Discovery = response.json().await?;
    ensure!(discovery.issuer == ISSUER, "Unexpected OIDC issuer");
    let jwks_url = Url::parse(&discovery.jwks_uri)?;
    ensure!(
        jwks_url.scheme() == "https"
            && jwks_url.host_str() == Some("auth.openai.com")
            && jwks_url.username().is_empty()
            && jwks_url.password().is_none(),
        "Unexpected JWKS origin"
    );
    let response = client.get(jwks_url).send().await?;
    ensure!(
        response.status().is_success(),
        "OIDC signing-key discovery failed"
    );
    let keys: JwkSet = response.json().await?;
    validate_identity(token, client_id, nonce, subject, &keys)
}

pub fn validate_identity(
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    subject: Option<&str>,
    keys: &JwkSet,
) -> Result<Identity> {
    let header = decode_header(token).context("Invalid ID token header")?;
    ensure!(
        matches!(
            header.alg,
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::ES256
                | Algorithm::ES384
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512
                | Algorithm::EdDSA
        ),
        "ID token must use an asymmetric signing algorithm"
    );
    let kid = header.kid.context("ID token has no signing-key ID")?;
    let jwk = keys
        .find(&kid)
        .context("ID token signing key is not published by issuer")?;
    let key = DecodingKey::from_jwk(jwk)?;
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[client_id]);
    validation.set_issuer(&[ISSUER]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    validation.leeway = 30;
    let identity = decode::<Identity>(token, &key, &validation)
        .map_err(|_| anyhow::anyhow!("ID token signature or claims validation failed"))?
        .claims;
    ensure!(!identity.sub.is_empty(), "ID token subject is missing");
    if let Some(nonce) = nonce {
        ensure!(
            identity.nonce.as_deref() == Some(nonce),
            "ID token nonce mismatch"
        );
    }
    if let Some(subject) = subject {
        ensure!(
            identity.sub == subject,
            "Signed-in identity differs from the selected account"
        );
    }
    Ok(identity)
}

fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub async fn login(
    paths: &Paths,
    config: &Config,
    credential: &str,
    tx: &mpsc::Sender<UiEvent>,
    cancel: &CancellationToken,
) -> Result<()> {
    tokio::select! {_=cancel.cancelled()=>bail!("ChatGPT sign-in canceled"),result=login_inner(paths,config,credential,tx)=>result}
}
async fn login_inner(
    paths: &Paths,
    config: &Config,
    credential: &str,
    tx: &mpsc::Sender<UiEvent>,
) -> Result<()> {
    ensure!(valid_id(credential), "Invalid credential reference");
    let dir = auth_dir(paths)?;
    let host_file = dir.join("host.json");
    let host = {
        let host_lock = dir.join("host.lock");
        let _lock = tokio::task::spawn_blocking(move || lock_file(&host_lock, true)).await??;
        match fs::read(&host_file) {
            Ok(bytes) => {
                serde_json::from_slice::<String>(&bytes).context("Invalid local agent host ID")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let value = format!("urn:uuid:{}", Uuid::new_v4());
                atomic_write(&host_file, &serde_json::to_vec(&value)?, None)?;
                value
            }
            Err(e) => return Err(e.into()),
        }
    };
    let registration_path = dir.join(format!("{credential}.registration.json"));
    let previous = if credential_path(paths, credential)?.exists() {
        match load(paths, credential)? {
            Credential::Chatgpt { session } => Some(session),
            _ => bail!("Credential reference already belongs to an API key"),
        }
    } else {
        None
    };
    let registration: Option<Registration> = match fs::read(&registration_path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let issued = previous
        .as_ref()
        .map(|p| p.client_id.as_str())
        .or_else(|| registration.as_ref().map(|r| r.client_id.as_str()));
    let subject = previous
        .as_ref()
        .map(|p| p.subject.as_str())
        .or_else(|| registration.as_ref().and_then(|r| r.subject.as_deref()));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let state = random_secret();
    let nonce = random_secret();
    let verifier = random_secret();
    let challenge = pkce_challenge(&verifier);
    let mut authorization = Url::parse(AUTHORIZE)?;
    {
        let mut query = authorization.query_pairs_mut();
        query.extend_pairs([
            ("client_id", issued.unwrap_or("dynamic_agent_client")),
            ("ext_agent_host_id", host.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            (
                "scope",
                "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
            ),
            ("resource", RESOURCE),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ]);
        if issued.is_none() {
            query.append_pair("agent_name_hint", "ruvx");
        }
    }
    tx.send(UiEvent::Line(
        "Continue with ChatGPT · Finish registration/consent in your browser. Ctrl+C cancels."
            .into(),
    ))
    .await?;
    let browser = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let opened = tokio::process::Command::new(browser)
        .arg(authorization.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success());
    // No id_token_hint is attached, so this fallback URL does not contain a token.
    if !opened {
        tx.send(UiEvent::Line(format!(
            "Open this sign-in URL in a browser on this machine:\n{authorization}"
        )))
        .await?;
    }
    let callback = tokio::time::timeout(
        Duration::from_secs(300),
        receive_callback(&listener, &state),
    )
    .await
    .context("ChatGPT sign-in timed out")??;
    let client_id = match (issued, callback.client_id.as_deref()) {
        (Some(expected), Some(actual)) => {
            ensure!(
                expected == actual,
                "OAuth callback changed issued client ID"
            );
            expected.to_owned()
        }
        (Some(expected), None) => expected.to_owned(),
        (None, Some(actual)) => {
            ensure!(
                !actual.is_empty() && actual != "dynamic_agent_client" && actual.len() < 512,
                "Invalid issued client ID"
            );
            actual.into()
        }
        (None, None) => bail!("Registration callback did not include an issued client ID"),
    };
    atomic_write(
        &registration_path,
        &serde_json::to_vec(&Registration {
            client_id: client_id.clone(),
            subject: subject.map(str::to_owned),
        })?,
        None,
    )?;
    let client = provider::client(config)?;
    let token = token_request(
        &client,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &callback.code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let id_token = token.id_token.context("OAuth response has no ID token")?;
    let identity =
        validate_remote_identity(&client, &id_token, &client_id, Some(&nonce), subject).await?;
    let scopes: Vec<String> = token
        .scope
        .context("OAuth response has no granted scopes")?
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    ensure_scope(&scopes)?;
    let session = OAuthSession {
        issuer: ISSUER.into(),
        subject: identity.sub.clone(),
        email: identity.email,
        client_id: client_id.clone(),
        ext_agent_host_id: host,
        id_token,
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .context("OAuth response has no refresh token")?,
        scopes,
        expires_at: Utc::now()
            .timestamp()
            .checked_add(i64::try_from(token.expires_in)?)
            .context("Invalid expiry")?,
    };
    save(paths, credential, &Credential::Chatgpt { session })?;
    atomic_write(
        &registration_path,
        &serde_json::to_vec(&Registration {
            client_id,
            subject: Some(identity.sub),
        })?,
        None,
    )?;
    Ok(())
}

struct Callback {
    code: String,
    client_id: Option<String>,
}
async fn receive_callback(listener: &TcpListener, state: &str) -> Result<Callback> {
    loop {
        let (mut stream, peer) = listener.accept().await?;
        ensure!(
            peer.ip().is_loopback(),
            "OAuth callback peer is not loopback"
        );
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        let read = async {
            loop {
                let n = stream.read(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..n]);
                ensure!(buffer.len() <= 8192, "OAuth callback request too large");
                if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        };
        match tokio::time::timeout(Duration::from_secs(5), read).await {
            Ok(result) => result?,
            Err(_) => continue,
        }
        let text = std::str::from_utf8(&buffer).context("Invalid OAuth callback HTTP")?;
        let mut first = text.lines().next().unwrap_or("").split_whitespace();
        let method = first.next();
        let target = first.next().unwrap_or("");
        if method != Some("GET") || !target.starts_with("/auth/callback?") {
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await?;
            continue;
        }
        let result = parse_callback(target, state);
        let body = if result.is_ok() {
            "Sign-in received. Return to ruvx; identity and plan permission are being verified."
        } else {
            "Sign-in was not accepted. Return to ruvx."
        };
        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            if result.is_ok() {
                "200 OK"
            } else {
                "400 Bad Request"
            },
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        return result;
    }
}
fn parse_callback(target: &str, state: &str) -> Result<Callback> {
    let url = Url::parse(&format!("http://127.0.0.1{target}"))?;
    ensure!(url.path() == "/auth/callback", "Unexpected callback path");
    let mut params = BTreeMap::new();
    for (k, v) in url.query_pairs() {
        ensure!(
            params.insert(k.into_owned(), v.into_owned()).is_none(),
            "Duplicate OAuth callback parameter"
        );
    }
    ensure!(
        params.get("state").map(String::as_str) == Some(state),
        "OAuth callback state mismatch"
    );
    ensure!(
        !params.contains_key("error"),
        "ChatGPT consent was denied or failed; no credentials were replaced"
    );
    let code = params
        .remove("code")
        .filter(|s| !s.is_empty())
        .context("OAuth callback has no authorization code")?;
    Ok(Callback {
        code,
        client_id: params.remove("client_id"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn callback_validation_and_pkce() {
        assert!(parse_callback("/auth/callback?state=wrong&code=secret", "right").is_err());
        assert!(parse_callback("/auth/callback?state=right&error=access_denied", "right").is_err());
        assert!(parse_callback("/auth/callback?state=right&state=right&code=x", "right").is_err());
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
    #[test]
    fn scope_gate_and_private_key_storage() {
        assert!(ensure_scope(&["openid".into()]).is_err());
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        save_key(&paths, "test", "secret-test-key").unwrap();
        let path = credential_path(&paths, "test").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(matches!(
            load(&paths, "test").unwrap(),
            Credential::ApiKey { .. }
        ));
    }
    #[test]
    fn reject_symmetric_id_tokens() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        let token = encode(
            &Header::default(),
            &json!({"sub":"x","exp":Utc::now().timestamp()+600,"iss":ISSUER,"aud":"app"}),
            &EncodingKey::from_secret(b"test"),
        )
        .unwrap();
        assert!(validate_identity(&token, "app", None, None, &JwkSet { keys: vec![] }).is_err());
    }

    #[test]
    fn rsa_signature_and_identity_claims() {
        use jsonwebtoken::{EncodingKey, Header, encode};
        let key = EncodingKey::from_rsa_pem(include_bytes!("../tests/fixtures/oidc-test-key.pem"))
            .unwrap();
        let keys: JwkSet =
            serde_json::from_str(include_str!("../tests/fixtures/oidc-test-jwks.json")).unwrap();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("ruvx-test".into());
        let claims = json!({"iss":ISSUER,"aud":"app","sub":"account-a","nonce":"nonce-a","exp":Utc::now().timestamp()+600});
        let token = encode(&header, &claims, &key).unwrap();
        assert_eq!(
            validate_identity(&token, "app", Some("nonce-a"), Some("account-a"), &keys)
                .unwrap()
                .sub,
            "account-a"
        );
        assert!(validate_identity(&token, "other-app", Some("nonce-a"), None, &keys).is_err());
        assert!(validate_identity(&token, "app", Some("wrong"), None, &keys).is_err());
        assert!(validate_identity(&token, "app", None, Some("account-b"), &keys).is_err());
        for (field, value) in [
            ("iss", json!("https://attacker.invalid")),
            ("exp", json!(Utc::now().timestamp() - 120)),
        ] {
            let mut invalid = claims.clone();
            invalid[field] = value;
            assert!(
                validate_identity(
                    &encode(&header, &invalid, &key).unwrap(),
                    "app",
                    None,
                    None,
                    &keys
                )
                .is_err()
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rotating_refresh_is_serialized_and_saved_atomically() {
        use axum::{
            Json, Router,
            extract::{Form, State},
            routing::post,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        async fn exchange(
            State(count): State<Arc<AtomicUsize>>,
            Form(form): Form<BTreeMap<String, String>>,
        ) -> Json<serde_json::Value> {
            assert_eq!(form["grant_type"], "refresh_token");
            assert_eq!(form["client_id"], "issued-client");
            assert_eq!(form["refresh_token"], "old-refresh");
            assert_eq!(form["resource"], RESOURCE);
            assert!(!form.contains_key("scope"));
            count.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            Json(
                json!({"access_token":"new-access","refresh_token":"rotated-refresh","expires_in":3600,"token_type":"Bearer"}),
            )
        }
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path());
        save(
            &paths,
            "oauth",
            &Credential::Chatgpt {
                session: OAuthSession {
                    issuer: ISSUER.into(),
                    subject: "account".into(),
                    email: None,
                    client_id: "issued-client".into(),
                    ext_agent_host_id: "host".into(),
                    id_token: "retained-id-token".into(),
                    access_token: "expired-access".into(),
                    refresh_token: "old-refresh".into(),
                    scopes: vec![INFERENCE_SCOPE.into()],
                    expires_at: 0,
                },
            },
        )
        .unwrap();
        let profile = Profile {
            kind: crate::config::ProviderKind::Chatgpt,
            label: String::new(),
            base_url: None,
            auth: AuthConfig::Chatgpt {
                credential: "oauth".into(),
            },
            models: BTreeMap::new(),
        };
        let count = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/token", post(exchange))
            .with_state(count.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let config = Config::default();
        let cancel = CancellationToken::new();
        let (a, b) = tokio::join!(
            resolve_using(&paths, &profile, &config, &cancel, &endpoint),
            resolve_using(&paths, &profile, &config, &cancel, &endpoint)
        );
        assert_eq!(a.unwrap().as_deref(), Some("new-access"));
        assert_eq!(b.unwrap().as_deref(), Some("new-access"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let Credential::Chatgpt { session } = load(&paths, "oauth").unwrap() else {
            panic!("wrong record")
        };
        assert_eq!(session.refresh_token, "rotated-refresh");
        assert_eq!(session.id_token, "retained-id-token");
        server.abort();
    }
}
