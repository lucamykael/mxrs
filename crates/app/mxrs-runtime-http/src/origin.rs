use axum::http::{HeaderMap, Uri, header};

use crate::{HttpError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let uri: Uri = value.parse().map_err(|_| HttpError::InvalidPublicOrigin)?;
        let scheme = uri.scheme_str().ok_or(HttpError::InvalidPublicOrigin)?;
        let authority = uri.authority().ok_or(HttpError::InvalidPublicOrigin)?;
        let port_suffix = authority.as_str().strip_prefix(authority.host());
        if !matches!(scheme, "http" | "https")
            || authority.as_str().contains('@')
            || authority.host().is_empty()
            || port_suffix.is_none_or(|suffix| !suffix.is_empty() && authority.port_u16().is_none())
            || uri
                .path_and_query()
                .is_some_and(|path| path.as_str() != "/")
            || value.contains('#')
        {
            return Err(HttpError::InvalidPublicOrigin);
        }
        Ok(Self {
            scheme: scheme.into(),
            host: authority.host().to_ascii_lowercase(),
            port: authority
                .port_u16()
                .unwrap_or(if scheme == "https" { 443 } else { 80 }),
        })
    }
}

pub(crate) fn request_allowed(headers: &HeaderMap, pinned: Option<&Origin>) -> bool {
    let sites = headers.get_all("sec-fetch-site");
    let mut sites = sites.iter();
    if let Some(site) = sites.next()
        && (!matches!(site.to_str(), Ok("same-origin" | "none")) || sites.next().is_some())
    {
        return false;
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let Some(value) = origins.next() else {
        // Non-browser JSON clients do not send Origin. Cross-site forms cannot
        // pass the JSON extractor, and this router never grants CORS preflight.
        return true;
    };
    if origins.next().is_some() {
        return false;
    }
    let Some(origin) = value
        .to_str()
        .ok()
        .and_then(|value| Origin::parse(value).ok())
    else {
        return false;
    };
    if let Some(pinned) = pinned {
        return origin == *pinned;
    }
    let ip_host = origin.host.trim_start_matches('[').trim_end_matches(']');
    if origin.host != "localhost"
        && !ip_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
    {
        return false;
    }
    let mut hosts = headers.get_all(header::HOST).iter();
    let Some(host) = hosts.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    if hosts.next().is_some() {
        return false;
    }
    Origin::parse(&format!("http://{host}")).is_ok_and(|expected| expected == origin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn origin_parsing_normalizes_defaults_and_rejects_non_origins() {
        for (left, right) in [
            ("http://EXAMPLE.com", "http://example.com:80/"),
            ("https://example.com", "https://example.com:443"),
            ("http://[::1]", "http://[::1]:80"),
        ] {
            assert_eq!(Origin::parse(left).unwrap(), Origin::parse(right).unwrap());
        }
        for value in [
            "null",
            "",
            "/relative",
            "https:/missing",
            "file://example.com",
            "https://",
            "http://:80",
            "https://example.com:bad",
            "https://example.com:99999",
            "https://example.com:",
            "https://user@example.com",
            "https://example.com/path",
            "https://example.com?q=x",
            "https://example.com#fragment",
            "https://example.com http://other.test",
        ] {
            assert!(Origin::parse(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn origin_checks_require_exact_scheme_authority_and_unambiguous_headers() {
        let mut headers = HeaderMap::new();
        assert!(request_allowed(&headers, None));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        assert!(!request_allowed(&headers, None));
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));
        assert!(request_allowed(&headers, None));
        for value in [
            "null",
            "https://localhost:8080",
            "http://localhost:8081",
            "http://attacker.test",
        ] {
            headers.insert(header::ORIGIN, value.parse().unwrap());
            assert!(!request_allowed(&headers, None), "accepted {value}");
        }
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("http://attacker.test"),
        );
        assert!(!request_allowed(&headers, None));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.append(header::HOST, HeaderValue::from_static("attacker.test"));
        assert!(!request_allowed(&headers, None));
        headers.insert(header::HOST, HeaderValue::from_bytes(&[0xff]).unwrap());
        assert!(!request_allowed(&headers, None));
        headers.insert(header::ORIGIN, HeaderValue::from_bytes(&[0xff]).unwrap());
        assert!(!request_allowed(&headers, None));
    }

    #[test]
    fn fetch_metadata_rejects_cross_site_same_site_and_ambiguous_requests() {
        let mut headers = HeaderMap::new();
        for (site, expected) in [
            ("same-origin", true),
            ("none", true),
            ("cross-site", false),
            ("same-site", false),
            ("unexpected", false),
        ] {
            headers.insert("sec-fetch-site", site.parse().unwrap());
            assert_eq!(request_allowed(&headers, None), expected);
        }
        headers.insert("sec-fetch-site", HeaderValue::from_static("same-origin"));
        headers.append("sec-fetch-site", HeaderValue::from_static("none"));
        assert!(!request_allowed(&headers, None));
        headers.insert("sec-fetch-site", HeaderValue::from_bytes(&[0xff]).unwrap());
        assert!(!request_allowed(&headers, None));
    }

    #[test]
    fn a_pinned_origin_does_not_trust_host_or_forwarding_headers() {
        let pinned = Origin::parse("https://app.example.com").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8080"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.example.com"),
        );
        assert!(request_allowed(&headers, Some(&pinned)));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.test"),
        );
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("attacker.test"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert!(!request_allowed(&headers, Some(&pinned)));
        headers.insert(header::HOST, HeaderValue::from_static("app.example.com"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.example.com"),
        );
        assert!(!request_allowed(&headers, None));
    }

    #[test]
    fn unpinned_origins_reject_rebinding_hosts_and_accept_only_loopback_http() {
        for (host, allowed) in [
            ("localhost:8080", true),
            ("127.0.0.1:8080", true),
            ("[::1]:8080", true),
            ("127.20.30.40:8080", true),
            ("attacker.example:8080", false),
            ("localhost.attacker.example:8080", false),
            ("192.168.1.10:8080", false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, host.parse().unwrap());
            headers.insert(header::ORIGIN, format!("http://{host}").parse().unwrap());
            assert_eq!(request_allowed(&headers, None), allowed, "{host}");
        }
    }
}
