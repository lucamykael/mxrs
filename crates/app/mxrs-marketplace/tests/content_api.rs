//! Exercises the client against responses recorded from the real Marketplace
//! Content API (`tests/fixtures/`, captured 2026-09-14 for Community Commons,
//! content ID 170).
//!
//! Recorded rather than live so the suite needs neither a credential nor
//! connectivity: a test that only runs for whoever holds a token is a test
//! that does not run. The live path has one opt-in test at the bottom.

use std::cell::RefCell;
use std::path::Path;

use mxrs_marketplace::{
    ContentApi, Download, MarketplaceError, Pat, SearchQuery, Transport,
    credentials::{PAT_ENV, PAT_FILE_ENV},
};

/// Replays recorded bodies and records what was asked for, so the tests can
/// assert on the URL the client built as well as on what it parsed.
#[derive(Default)]
struct Recorded {
    responses: Vec<(String, String)>,
    requests: RefCell<Vec<String>>,
}

impl Recorded {
    fn with(mut self, fragment: &str, fixture: &str) -> Self {
        let body = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(format!("{fixture}.json")),
        )
        .unwrap();
        self.responses.push((fragment.to_string(), body));
        self
    }

    fn requests(&self) -> Vec<String> {
        self.requests.borrow().clone()
    }
}

impl Transport for Recorded {
    fn get(&self, url: &str, authorization: &str) -> mxrs_marketplace::Result<String> {
        // Every request must carry the Marketplace scheme; an anonymous call
        // would silently return public-only results.
        assert!(
            authorization.starts_with("MxToken "),
            "unexpected scheme: {authorization}"
        );
        self.requests.borrow_mut().push(url.to_string());
        self.responses
            .iter()
            .find(|(fragment, _)| url.contains(fragment.as_str()))
            .map(|(_, body)| body.clone())
            .ok_or_else(|| MarketplaceError::Status {
                status: 404,
                url: url.to_string(),
            })
    }

    fn download(
        &self,
        url: &str,
        authorization: Option<&str>,
        destination: &Path,
    ) -> mxrs_marketplace::Result<Download> {
        self.requests
            .borrow_mut()
            .push(format!("DOWNLOAD {url} auth={}", authorization.is_some()));
        // Mirrors the real service: the Marketplace endpoint redirects to the
        // file host, which serves the bytes.
        if url.contains("marketplace.mendix.com") {
            return Ok(Download::Redirect(
                "https://files.appstore.mendix.com/signed/package.mpk?sig=abc".into(),
            ));
        }
        std::fs::write(destination, b"PK\x03\x04fake-mpk").unwrap();
        Ok(Download::Written(11))
    }
}

fn api(transport: Recorded) -> ContentApi<Recorded> {
    ContentApi::new(transport, Pat::new("test-token").unwrap())
}

#[test]
fn a_search_parses_the_real_response_shape_including_the_name_on_the_version() {
    let client = api(Recorded::default().with("/content?", "content-search"));
    let results = client
        .search(&SearchQuery {
            name: Some("Community Commons".into()),
            limit: Some(5),
            ..SearchQuery::default()
        })
        .unwrap();

    assert_eq!(results.len(), 1);
    let content = &results[0];
    assert_eq!(content.content_id, 170);
    assert_eq!(content.publisher, "Mendix");
    assert_eq!(content.content_type, "Module");
    assert!(content.is_company_approved);
    assert!(!content.is_private);
    // The display name lives on `latestVersion`, not on the content record —
    // the reason a name search has to reach into the version.
    assert_eq!(content.name(), Some("Community Commons"));
    assert_eq!(
        content.latest_version.as_ref().unwrap().version_number,
        "11.5.1"
    );
}

#[test]
fn resolving_picks_the_newest_compatible_version_and_builds_its_download_url() {
    let client = api(Recorded::default()
        .with("/content/170/versions", "versions-170")
        .with("/content/170", "content-170"));
    let package = client.resolve("170", None, Some("11.12.1")).unwrap();

    assert_eq!(package.name(), "Community Commons");
    assert_eq!(package.version.version_number, "11.5.1");
    assert_eq!(
        package.download_url(),
        "https://marketplace.mendix.com/v1/versions/9dc402fd-e38d-420d-abbe-9fe0ecad6f82/download"
    );
    // A numeric identifier must not be looked up through the name search.
    let requests = client_requests(&client);
    assert!(
        requests.iter().any(|url| url.contains("/content/170")),
        "{requests:?}"
    );
    assert!(
        requests.iter().all(|url| !url.contains("/content?name=")),
        "{requests:?}"
    );
    // The Mendix target reaches the API as a filter rather than being checked
    // only client-side.
    assert!(
        requests
            .iter()
            .any(|url| url.contains("supportedMendixVersion=11.12.1")),
        "{requests:?}"
    );
}

#[test]
fn an_exact_version_can_be_requested_and_an_unknown_one_is_named() {
    let client = api(Recorded::default()
        .with("/content/170/versions", "versions-170")
        .with("/content/170", "content-170"));
    let package = client.resolve("170", Some("11.5.0"), None).unwrap();
    assert_eq!(package.version.version_number, "11.5.0");

    assert!(matches!(
        client.resolve("170", Some("99.0.0"), None),
        Err(MarketplaceError::NoSuchVersion { requested, .. }) if requested == "99.0.0"
    ));
}

#[test]
fn an_incompatible_mendix_target_is_refused_before_anything_is_downloaded() {
    let client = api(Recorded::default()
        .with("/content/170/versions", "versions-170")
        .with("/content/170", "content-170"));
    // Community Commons 11.5.1 declares a floor of Mendix 10.24.0.
    assert!(matches!(
        client.resolve("170", Some("11.5.1"), Some("9.24.0")),
        Err(MarketplaceError::Incompatible { minimum, requested, .. })
            if minimum == "10.24.0" && requested == "9.24.0"
    ));
}

#[test]
fn a_name_that_matches_nothing_or_several_entries_is_an_error_rather_than_a_guess() {
    let empty = api(Recorded::default().with("/content?", "empty-items"));
    assert!(matches!(
        empty.find("No Such Component"),
        Err(MarketplaceError::NotFound(name)) if name == "No Such Component"
    ));

    let many = api(Recorded::default().with("/content?", "content-search-ambiguous"));
    assert!(matches!(
        many.find("Commons"),
        Err(MarketplaceError::Ambiguous(name, count)) if name == "Commons" && count == 2
    ));
}

#[test]
fn a_download_url_outside_the_marketplace_never_receives_the_credential() {
    let client = api(Recorded::default()
        .with("/content/170/versions", "versions-170")
        .with("/content/170", "content-170"));
    let mut package = client.resolve("170", None, None).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("package.mpk");

    // The Marketplace endpoint is fetched with credentials; the file host it
    // redirects to serves a pre-signed URL and must NOT receive the token.
    let written = client.download(&package, &destination).unwrap();
    assert_eq!(written, 11);
    assert!(destination.exists());
    let downloads: Vec<String> = client_requests(&client)
        .into_iter()
        .filter(|entry| entry.starts_with("DOWNLOAD"))
        .collect();
    assert_eq!(downloads.len(), 2, "{downloads:?}");
    assert!(
        downloads[0].contains("marketplace.mendix.com") && downloads[0].ends_with("auth=true"),
        "{downloads:?}"
    );
    assert!(
        downloads[1].contains("files.appstore.mendix.com") && downloads[1].ends_with("auth=false"),
        "the account token must not reach the file host: {downloads:?}"
    );

    // A response naming another host is refused outright: not fetched
    // anonymously, and certainly not with the token.
    for hostile in [
        "https://evil.example/versions/x/download",
        "https://marketplace.mendix.com.evil.example/x",
        "https://evil.example@marketplace.mendix.com.attacker.test/x",
        "http://marketplace.mendix.com/x",
    ] {
        package.version.download_url = Some(hostile.to_string());
        assert!(
            matches!(
                client.download(&package, &destination),
                Err(MarketplaceError::UntrustedHost(_))
            ),
            "{hostile} was accepted"
        );
    }
}

#[test]
fn a_malformed_body_names_the_url_instead_of_panicking() {
    struct Garbage;
    impl Transport for Garbage {
        fn get(&self, _url: &str, _authorization: &str) -> mxrs_marketplace::Result<String> {
            Ok("<html>not json</html>".into())
        }
        fn download(
            &self,
            _url: &str,
            _authorization: Option<&str>,
            _destination: &Path,
        ) -> mxrs_marketplace::Result<Download> {
            unreachable!("not exercised")
        }
    }
    let client = ContentApi::new(Garbage, Pat::new("t").unwrap());
    assert!(matches!(
        client.content("170"),
        Err(MarketplaceError::Parse { url, .. }) if url.contains("/content/170")
    ));
}

fn client_requests(client: &ContentApi<Recorded>) -> Vec<String> {
    client.transport().requests()
}

/// Opt-in live check against the real API. Ignored by default: the suite must
/// pass with no credential and no network.
///
/// Run with a token available:
///   `MXRS_MENDIX_PAT_FILE=~/.ssh/pat_mendix cargo test -p mxrs-marketplace -- --ignored`
#[test]
#[ignore = "requires a Mendix PAT and network access"]
fn the_live_api_still_answers_the_shape_these_fixtures_were_recorded_from() {
    let credentials = mxrs_marketplace::Credentials::default();
    let pat = match credentials.resolve() {
        Ok(pat) => pat,
        Err(error) => panic!("set {PAT_ENV} or {PAT_FILE_ENV}: {error}"),
    };
    let client = ContentApi::new(mxrs_marketplace::ureq_transport::UreqTransport::new(), pat);

    let content = client
        .content("170")
        .expect("content 170 is Community Commons");
    assert_eq!(content.content_id, 170);
    assert_eq!(content.name(), Some("Community Commons"));

    let versions = client.versions("170", Some("11.12.1")).unwrap();
    assert!(!versions.is_empty());
    assert!(
        versions
            .iter()
            .all(|version| !version.version_id.is_empty())
    );
}
