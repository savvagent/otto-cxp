//! The platform's `/oauth/token` and `/oauth/revoke`, server to server, PKCE,
//! and the sealed in-flight sign-in the oauth cookie carries.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use otto_tenant::crypto::Cipher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// RFC 7636 `S256`: base64url, unpadded, of the SHA-256 of the verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// What `/auth/login` remembers for `/auth/callback`, sealed in a cookie with
/// the OAuth-flow key (never the session-token key).
#[derive(Serialize, Deserialize)]
pub(crate) struct OauthFlow {
    pub(crate) state: String,
    pub(crate) verifier: String,
    /// A console path to land on afterwards, already checked by
    /// [`super::routes::safe_next`].
    pub(crate) next: Option<String>,
    /// Unix seconds. The cookie's `Max-Age` is advisory; this is enforced.
    pub(crate) exp: i64,
}

// Not derived: the verifier is a secret until the code is redeemed.
impl std::fmt::Debug for OauthFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthFlow")
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl OauthFlow {
    pub(crate) fn seal(&self, cipher: &Cipher) -> Option<String> {
        let json = serde_json::to_vec(self).ok()?;
        let sealed = cxp_core::sealed::seal(cipher, &json).ok()?;
        Some(URL_SAFE_NO_PAD.encode(sealed))
    }

    /// `None` for anything that is not an unexpired flow we sealed.
    pub(crate) fn open(cipher: &Cipher, value: &str, now: i64) -> Option<Self> {
        let sealed = URL_SAFE_NO_PAD.decode(value).ok()?;
        let json = cxp_core::sealed::open(cipher, &sealed).ok()?;
        let flow: Self = serde_json::from_slice(&json).ok()?;
        (flow.exp > now).then_some(flow)
    }
}

/// What the platform issues for an authorization code or a refresh token.
#[derive(Deserialize)]
pub(crate) struct TokenSet {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    /// Seconds until `access_token` expires.
    #[serde(default)]
    pub(crate) expires_in: Option<i64>,
}

// Not derived: neither token may reach a log line.
impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) enum TokenError {
    /// The code or refresh token is spent, expired, revoked, or never valid.
    InvalidGrant,
    /// Any other refusal (`invalid_client`, `invalid_target`, ...): the client
    /// registration or this configuration is wrong, not the person's doing.
    Rejected(String),
    /// No usable answer: transport failure, timeout, 5xx, 429, or a body that
    /// is not a token response.
    Unavailable(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
}

/// How long one call to the platform's token or revoke endpoint may take.
pub(crate) const PLATFORM_TIMEOUT: Duration = Duration::from_secs(10);

/// The platform's `/oauth/token` and `/oauth/revoke`. The console is a public
/// client: `client_id` is its whole identity, and PKCE is what binds a code to
/// the browser that asked for it.
pub(crate) struct PlatformOAuth {
    http: reqwest::Client,
    token_url: String,
    revoke_url: String,
}

impl PlatformOAuth {
    pub(crate) fn new(platform_url: &str) -> Self {
        let base = platform_url.trim_end_matches('/');
        Self {
            http: reqwest::Client::builder()
                .timeout(PLATFORM_TIMEOUT)
                // The token endpoint never redirects; following one would send
                // a code or a refresh token wherever it pointed.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("the TLS backend initialises"),
            token_url: format!("{base}/oauth/token"),
            revoke_url: format!("{base}/oauth/revoke"),
        }
    }

    pub(crate) async fn exchange_code(
        &self,
        client_id: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        resource: &str,
    ) -> Result<TokenSet, TokenError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", code_verifier),
            ("client_id", client_id),
            ("resource", resource),
        ])
        .await
    }

    pub(crate) async fn refresh(
        &self,
        client_id: &str,
        refresh_token: &str,
        resource: &str,
    ) -> Result<TokenSet, TokenError> {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("resource", resource),
        ])
        .await
    }

    /// Revoke a refresh token. Only not reaching the platform is a failure,
    /// which the caller logs and moves past: signing out must work while the
    /// platform is down.
    pub(crate) async fn revoke(&self, client_id: &str, token: &str) -> Result<(), TokenError> {
        self.http
            .post(&self.revoke_url)
            .form(&[("token", token), ("client_id", client_id)])
            .send()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        Ok(())
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<TokenSet, TokenError> {
        let response = self
            .http
            .post(&self.token_url)
            .form(form)
            .send()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| TokenError::Unavailable(e.without_url().to_string()))?;
        if status.is_success() {
            return serde_json::from_slice(&body)
                .map_err(|_| TokenError::Unavailable("an unreadable token response".into()));
        }
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(TokenError::Unavailable(format!("HTTP {status}")));
        }
        let error = serde_json::from_slice::<ErrorBody>(&body)
            .map(|b| b.error)
            .unwrap_or_default();
        match error.as_str() {
            "invalid_grant" => Err(TokenError::InvalidGrant),
            // The platform's own error code, from a fixed vocabulary; logged,
            // never shown to the person.
            "" => Err(TokenError::Rejected(format!("HTTP {status}"))),
            other => Err(TokenError::Rejected(
                other
                    .chars()
                    .take(64)
                    .filter(|c| c.is_ascii_graphic())
                    .collect(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> Cipher {
        Cipher::from_base64_key(&base64::engine::general_purpose::STANDARD.encode([byte; 32]))
            .unwrap()
    }

    /// RFC 7636 appendix B's worked example.
    #[test]
    fn the_pkce_challenge_matches_the_rfc_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_flow_survives_its_cookie_and_expires() {
        let flow = OauthFlow {
            state: "s".into(),
            verifier: "v".into(),
            next: Some("/console/tickets".into()),
            exp: 1_000,
        };
        let sealed = flow.seal(&cipher(5)).unwrap();
        let back = OauthFlow::open(&cipher(5), &sealed, 999).unwrap();
        assert_eq!((back.state.as_str(), back.verifier.as_str()), ("s", "v"));
        assert_eq!(back.next.as_deref(), Some("/console/tickets"));
        assert!(
            OauthFlow::open(&cipher(5), &sealed, 1_000).is_none(),
            "expired"
        );
        assert!(
            OauthFlow::open(&cipher(6), &sealed, 0).is_none(),
            "wrong key"
        );
        for junk in ["garbage", "", "AAAA", &URL_SAFE_NO_PAD.encode([0u8; 12])] {
            assert!(OauthFlow::open(&cipher(5), junk, 0).is_none(), "{junk}");
        }
        assert!(!format!("{back:?}").contains("verifier"));
    }
}
