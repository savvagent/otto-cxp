//! The connector install page: what a person who opens `/` in a browser gets
//! instead of the JSON 404 (#25). Savvagent CXP has no web console; people use it
//! from an AI assistant as a custom connector, and this page says how.
//!
//! Only a browser gets it: [`wants_page`] needs an `Accept` that names
//! `text/html` with q > 0 and no `Authorization` header, so curl (`*/*`),
//! agents (`application/json`), and anything presenting credentials keep the
//! JSON 404 they rely on.
//!
//! The page is rendered once, at startup, from [`TEMPLATE`]: the connector
//! address is `CXP_RESOURCE_URI` exactly as configured (it is the token
//! audience, so it is never normalised) and the links are on the
//! `CXP_PLATFORM_URL` origin, all HTML-escaped. Nothing in the `<style>` or
//! `<script>` block is substituted, so [`CONTENT_SECURITY_POLICY`] can pin both
//! by hash whatever the configuration; the copy script reads the address from
//! the element's text instead of carrying it. A test recomputes both hashes,
//! so an edit to either block fails the build instead of silently breaking the
//! page under its own CSP.

use anyhow::{Context, Result};
use axum::body::{Body, Bytes};
use axum::response::Response;
use http::{header, HeaderMap, HeaderValue};

/// The request headers that choose between this page and the JSON 404 at
/// `/` (see [`wants_page`]). Both answers carry it, so no shared cache hands
/// one to a request that should get the other (#32).
pub const VARY: &str = "Accept, Authorization";

/// The page, with `{{CONNECTOR_URL}}`, `{{GUIDE_URL}}`, and `{{CONSOLE_URL}}`
/// still in it.
const TEMPLATE: &str = include_str!("install_page.html");

/// Admits exactly the template's one `<style>` and one `<script>` block, and
/// nothing else: the page fetches nothing, submits nothing, and is never
/// framed.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
     style-src 'sha256-FczW5OVrT2mS0VV+4I1vUxN60McgpyXt7vmlNbtNfGc='; \
     script-src 'sha256-NTLwd0JRu6YZoG8Wgdva2igwwGrtMuA+ECWFiFJOpkM='; \
     base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The rendered page, shared by every request.
#[derive(Clone)]
pub struct InstallPage {
    html: Bytes,
}

impl InstallPage {
    /// Renders the page for this deployment. Fails only on a platform URL
    /// that does not parse, which configuration has already refused
    /// (`CXP_PLATFORM_URL` must be http(s), so its origin is never opaque).
    ///
    /// The configured address is substituted last, so text in it that looks
    /// like a placeholder is never replaced.
    pub fn new(resource_uri: &str, platform_url: &str) -> Result<Self> {
        let origin = url::Url::parse(platform_url)
            .context("CXP_PLATFORM_URL is not a URL")?
            .origin()
            .ascii_serialization();
        let html = TEMPLATE
            .replace(
                "{{GUIDE_URL}}",
                &escape(&format!("{origin}/products/cxp#connect")),
            )
            .replace("{{CONSOLE_URL}}", &escape(&format!("{origin}/")))
            .replace("{{CONNECTOR_URL}}", &escape(resource_uri));
        Ok(Self {
            html: Bytes::from(html),
        })
    }

    /// `200` with the page, or with its headers alone for `HEAD`.
    pub fn response(&self, head: bool) -> Response {
        let body = if head {
            Body::empty()
        } else {
            Body::from(self.html.clone())
        };
        let mut res = Response::new(body);
        let headers = res.headers_mut();
        for (name, value) in [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            // The same URL answers JSON to programs: no cache may hand this
            // to a request with another Accept or with credentials.
            (header::VARY, VARY),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
        ] {
            headers.insert(name, HeaderValue::from_static(value));
        }
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(self.html.len()));
        res
    }
}

/// A browser asking for a page: some `Accept` range is exactly `text/html`
/// with q > 0, and the request carries no `Authorization` at all (a caller
/// presenting credentials is a program). `text/*` and `*/*` do not count:
/// curl sends `*/*`. An `Accept` value that is not visible ASCII is ignored,
/// which leaves the JSON 404.
pub fn wants_page(headers: &HeaderMap) -> bool {
    !headers.contains_key(header::AUTHORIZATION)
        && headers
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(accepts_html)
}

/// One media range: `text/html`, case-insensitively, whose `q` (default 1)
/// parses as a float in (0, 1]. This is looser than RFC 9110's `qvalue`
/// grammar (it takes `.5` or `1e-1`), which only ever means a browser-like
/// request gets the page. A `q` that does not parse makes the range not
/// count, which falls back to the JSON 404.
fn accepts_html(range: &str) -> bool {
    let mut parts = range.split(';');
    let media = parts.next().unwrap_or_default().trim();
    if !media.eq_ignore_ascii_case("text/html") {
        return false;
    }
    for param in parts {
        if let Some((name, value)) = param.split_once('=') {
            if name.trim().eq_ignore_ascii_case("q") {
                return value
                    .trim()
                    .parse::<f32>()
                    .is_ok_and(|q| q > 0.0 && q <= 1.0);
            }
        }
    }
    true
}

/// Escapes text for an HTML element or a double-quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    /// The text between `<tag>` and `</tag>`, which must occur exactly once
    /// and carry no attributes (an attribute would change what is hashed).
    fn block(tag: &str) -> &'static str {
        assert_eq!(
            TEMPLATE.matches(&format!("<{tag}")).count(),
            1,
            "one <{tag}>"
        );
        let open = format!("<{tag}>");
        let start = TEMPLATE.find(&open).expect("no attributes") + open.len();
        let end = TEMPLATE.find(&format!("</{tag}>")).unwrap();
        &TEMPLATE[start..end]
    }

    fn hash(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(text.as_bytes()))
    }

    /// Editing the page's style or script without updating the CSP would
    /// leave the page unstyled or the copy buttons dead in every browser,
    /// with nothing else failing. This recomputes both hashes.
    #[test]
    fn the_csp_pins_exactly_the_embedded_style_and_script() {
        let expected = format!(
            "default-src 'none'; style-src 'sha256-{}'; script-src 'sha256-{}'; \
             base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            hash(block("style")),
            hash(block("script")),
        );
        assert_eq!(CONTENT_SECURITY_POLICY, expected);
    }

    /// The CSP admits no inline handler or style attribute, so one would
    /// silently do nothing. A smoke check over the words of the template, not
    /// an HTML parse: it catches the usual `onclick="..."` and `style="..."`.
    #[test]
    fn the_template_has_no_inline_handlers_or_style_attributes() {
        let template = TEMPLATE.to_ascii_lowercase();
        assert!(!template.contains("style="), "style attribute");
        assert!(!template.contains("javascript:"), "javascript: URL");
        for word in template.split(|c: char| c.is_whitespace() || c == '<' || c == '/') {
            assert!(
                !(word.starts_with("on") && word.contains('=')),
                "inline handler: {word}"
            );
        }
    }

    /// `include_str!` embeds the file's bytes, so a CRLF checkout would change
    /// what the CSP hashes (`.gitattributes` pins the file to LF).
    #[test]
    fn the_template_has_lf_line_endings() {
        assert!(!TEMPLATE.contains('\r'));
    }

    /// The script's hash must not depend on configuration, and both copy
    /// outcomes are announced from it.
    #[test]
    fn the_script_carries_no_placeholder_and_both_messages() {
        let script = block("script");
        assert!(!script.contains("{{"), "{script}");
        assert!(!block("style").contains("{{"));
        assert!(script.contains("\"Copied.\""));
        assert!(script.contains("\"Selected. Press Ctrl+C to copy.\""));
    }

    fn html(page: &InstallPage) -> String {
        String::from_utf8(page.html.to_vec()).unwrap()
    }

    #[test]
    fn rendering_substitutes_escaped_config() {
        let page = InstallPage::new(
            "https://cxp.example.com/mcp?a=1&b=<x>\"'",
            "https://platform.example.com/some/path",
        )
        .unwrap();
        let html = html(&page);
        assert!(!html.contains("{{"), "a placeholder survived");
        let escaped = "https://cxp.example.com/mcp?a=1&amp;b=&lt;x&gt;&quot;&#39;";
        // The address field, the Claude Code command, the developer note.
        assert_eq!(html.matches(escaped).count(), 3, "{html}");
        assert!(html.contains(&format!(
            "claude mcp add --transport http savvagent-cxp {escaped}</code>"
        )));
        assert!(html.contains("href=\"https://platform.example.com/products/cxp#connect\""));
        assert!(html.contains("href=\"https://platform.example.com/\""));
        // The style and script are untouched, so the CSP still holds.
        assert!(html.contains(block("style")) && html.contains(block("script")));
    }

    #[test]
    fn the_address_is_shown_exactly_as_configured() {
        let page =
            InstallPage::new("https://cxp.example.com/mcp/", "http://127.0.0.1:4000").unwrap();
        let html = html(&page);
        assert!(html.contains("<code id=\"address\">https://cxp.example.com/mcp/</code>"));
        assert!(html.contains("href=\"http://127.0.0.1:4000/\""));
    }

    /// The address goes in last, so placeholder-like text in it is shown,
    /// not substituted.
    #[test]
    fn placeholder_text_in_the_address_is_left_alone() {
        let page = InstallPage::new(
            "https://cxp.example.com/{{GUIDE_URL}}/mcp",
            "https://platform.example.com",
        )
        .unwrap();
        assert!(html(&page)
            .contains("<code id=\"address\">https://cxp.example.com/{{GUIDE_URL}}/mcp</code>"));
    }

    #[test]
    fn an_unparsable_platform_url_is_refused() {
        assert!(InstallPage::new("https://cxp.example.com/mcp", "not a url").is_err());
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn only_a_browser_without_credentials_wants_the_page() {
        let browser = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
        for (accept, want) in [
            (browser, true),
            ("text/html", true),
            ("TEXT/HTML", true),
            (" text/html ; q=0.5", true),
            ("text/html;level=1;q=0.5", true),
            ("text/html;Q=1", true),
            ("*/*", false),
            ("text/*", false),
            ("application/json", false),
            ("text/html;q=0", false),
            ("text/html;q=0.0", false),
            ("text/html;q=abc", false),
            ("text/html;q=2", false),
            ("text/htmlx", false),
            ("", false),
        ] {
            assert_eq!(
                wants_page(&headers(&[("accept", accept)])),
                want,
                "{accept:?}"
            );
        }
        assert!(!wants_page(&headers(&[])), "no Accept");
        // Several Accept headers are read together.
        assert!(wants_page(&headers(&[
            ("accept", "application/json"),
            ("accept", "text/html")
        ])));
        // Any Authorization, even an empty one, is a program.
        for auth in ["Bearer otto_at_x", "Basic eA==", ""] {
            assert!(
                !wants_page(&headers(&[("accept", browser), ("authorization", auth)])),
                "{auth:?}"
            );
        }
    }

    #[test]
    fn the_response_carries_the_headers_and_head_has_no_body() {
        let page = InstallPage::new(
            "https://cxp.example.com/mcp",
            "https://platform.example.com",
        )
        .unwrap();
        for head in [false, true] {
            let res = page.response(head);
            assert_eq!(res.status(), http::StatusCode::OK);
            let h = res.headers();
            assert_eq!(h["content-type"], "text/html; charset=utf-8");
            assert_eq!(h["cache-control"], "public, max-age=3600");
            assert_eq!(h["vary"], "Accept, Authorization");
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(h["referrer-policy"], "no-referrer");
            assert_eq!(h["content-security-policy"], CONTENT_SECURITY_POLICY);
            assert_eq!(h["content-length"], page.html.len().to_string().as_str());
        }
    }
}
