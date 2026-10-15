//! The console's two cookies, and the same-origin check on cookie writes.
//!
//! - **`__Host-cxp_session`**: 32 random bytes, base64url, and nothing else. A
//!   lookup key with no claims in it; only its SHA-256 is stored.
//! - **`__Host-cxp_oauth`**: the in-flight sign-in (`state`, the PKCE
//!   verifier, where to land), sealed with the OAuth-flow key, for ten
//!   minutes. It makes the callback the answer to *this browser's* request
//!   (login CSRF) and keeps the verifier off the URL.
//!
//! Both are `HttpOnly; Secure; SameSite=Lax; Path=/` with no `Domain`, which
//! the `__Host-` prefix makes a browser enforce: a sibling subdomain cannot
//! plant them. `Lax` rather than `Strict` because the callback is a cross-site
//! top-level navigation (back from the platform) that must carry the oauth
//! cookie; `Lax` still keeps the session cookie off cross-site subresource
//! requests and POSTs, and the same-origin check on writes
//! ([`same_origin`]) covers same-site siblings. On a plain-http loopback
//! public URL (local development) the names drop `__Host-` and the cookies
//! drop `Secure`, which `__Host-` requires.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use http::{header, HeaderMap, HeaderValue, Method};
use rand::RngCore;

#[derive(Clone, Copy)]
pub(crate) struct Cookies {
    /// `__Host-` names and `Secure`: every deployment but plain-http loopback.
    pub(crate) secure: bool,
}

impl Cookies {
    pub(crate) fn session(self) -> &'static str {
        if self.secure {
            "__Host-cxp_session"
        } else {
            "cxp_session"
        }
    }

    pub(crate) fn oauth(self) -> &'static str {
        if self.secure {
            "__Host-cxp_oauth"
        } else {
            "cxp_oauth"
        }
    }

    /// A `Set-Cookie` value, or `None` if `value` holds a byte no header may
    /// (never, for the base64url values this module mints).
    pub(crate) fn set(self, name: &str, value: &str, max_age_secs: i64) -> Option<HeaderValue> {
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age={max_age_secs}"
        ))
        .ok()
    }

    /// Makes the browser drop `name`. Same attributes as when it was set: a
    /// clear that differs in `Path` or the `__Host-` requirements is ignored.
    pub(crate) fn clear(self, name: &'static str) -> HeaderValue {
        let secure = if self.secure { "; Secure" } else { "" };
        // Every part is a fixed ASCII string.
        HeaderValue::from_str(&format!(
            "{name}=; Path=/; HttpOnly{secure}; SameSite=Lax; Max-Age=0"
        ))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
    }
}

/// A random 32-byte value, base64url (43 characters).
pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn is_token_shaped(v: &str) -> bool {
    v.len() == 43
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The value of cookie `name`, matched on the exact name: a cookie called
/// `evil-__Host-cxp_session` is not ours.
pub(crate) fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| raw.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(n, _)| n.trim() == name)
        .map(|(_, v)| v.trim())
}

/// `scheme://host[:port]` of `url`, as a browser serializes an `Origin`.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    url.has_host().then(|| url.origin().ascii_serialization())
}

/// Whether a request that authenticates with the session cookie came from
/// this origin. Safe methods always pass. Otherwise `Origin` must equal the
/// public origin exactly (a sibling subdomain's differs, and so does the
/// `null` of a sandboxed frame); with no `Origin`, `Sec-Fetch-Site` must be
/// `same-origin`; with neither, the request is refused. Browsers send `Origin`
/// on every non-GET fetch, so there is no `Referer` fallback. An empty
/// `origin` (an unparsable public URL) refuses everything.
pub fn same_origin(origin: &str, method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    if origin.is_empty() {
        return false;
    }
    if let Some(value) = headers.get(header::ORIGIN) {
        return value.to_str().is_ok_and(|v| v == origin);
    }
    headers
        .get("sec-fetch-site")
        .is_some_and(|v| v == "same-origin")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_cookie_is_found_by_its_exact_name_and_only_if_well_formed() {
        let good = random_token();
        assert_eq!(good.len(), 43);
        assert!(is_token_shaped(&good));
        let h = headers(&[(
            "cookie",
            &format!("theme=dark; __Host-cxp_session={good}; x=1"),
        )]);
        assert_eq!(read_cookie(&h, "__Host-cxp_session"), Some(good.as_str()));
        let h = headers(&[("cookie", "evil-__Host-cxp_session=abc")]);
        assert_eq!(read_cookie(&h, "__Host-cxp_session"), None);
        for bad in ["short", "a'b", &"x".repeat(44), &"x".repeat(42)] {
            assert!(!is_token_shaped(bad), "{bad}");
        }
    }

    #[test]
    fn cookies_are_host_only_http_only_lax_and_secure_off_loopback() {
        let secure = Cookies { secure: true };
        let v = secure.set(secure.session(), "abc", 60).unwrap();
        assert_eq!(
            v,
            "__Host-cxp_session=abc; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=60"
        );
        assert_eq!(
            secure.clear(secure.oauth()),
            "__Host-cxp_oauth=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"
        );
        let dev = Cookies { secure: false };
        assert_eq!(
            dev.set(dev.session(), "abc", 60).unwrap(),
            "cxp_session=abc; Path=/; HttpOnly; SameSite=Lax; Max-Age=60"
        );
        for v in [secure.set("n", "v", 1), dev.set("n", "v", 1)] {
            assert!(!v.unwrap().to_str().unwrap().contains("Domain"));
        }
        // A value no header may carry is refused, not a panic.
        assert!(secure.set("n", "a\nb", 1).is_none());
    }

    #[test]
    fn the_origin_is_scheme_host_and_port() {
        assert_eq!(
            origin_of("https://cxp.example.com/").as_deref(),
            Some("https://cxp.example.com")
        );
        assert_eq!(
            origin_of("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(origin_of("nonsense"), None);
    }

    #[test]
    fn a_cookie_write_must_prove_its_origin() {
        const ORIGIN: &str = "https://cxp.example.com";
        let post = |h: &[(&str, &str)]| same_origin(ORIGIN, &Method::POST, &headers(h));
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(same_origin(
                ORIGIN,
                &m,
                &headers(&[("origin", "https://evil.test")])
            ));
        }
        assert!(post(&[("origin", ORIGIN)]));
        for bad in [
            "https://evil.test",
            "https://blog.cxp.example.com",
            "null",
            "http://cxp.example.com",
            "https://cxp.example.com:8443",
        ] {
            assert!(!post(&[("origin", bad)]), "{bad}");
        }
        assert!(post(&[("sec-fetch-site", "same-origin")]));
        for bad in ["same-site", "cross-site", "none"] {
            assert!(!post(&[("sec-fetch-site", bad)]), "{bad}");
        }
        assert!(!post(&[]), "no proof at all");
        assert!(
            !post(&[("referer", "https://cxp.example.com/x")]),
            "Referer is not proof"
        );
        // A wrong Origin is not rescued by fetch metadata.
        assert!(!post(&[
            ("origin", "https://evil.test"),
            ("sec-fetch-site", "same-origin")
        ]));
        for m in [Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(!same_origin(ORIGIN, &m, &headers(&[])), "{m}");
        }
        // An unparsable public URL fails closed.
        assert!(!same_origin("", &Method::POST, &headers(&[("origin", "")])));
    }
}
