use std::path::Path;

use crate::Result;

/// The only part of this crate that needs the network.
///
/// Everything else — URL construction, parameter validation, response parsing,
/// host checks — is above this line and therefore testable with a recorded
/// response and no credential. See the crate doc comment.
pub trait Transport {
    /// `authorization` is the full header value. Implementations must not log
    /// it, and must not follow a redirect to a host the caller did not
    /// authorize while still carrying it.
    fn get(&self, url: &str, authorization: &str) -> Result<String>;

    /// Performs **one** hop of a download, streaming to `destination`.
    ///
    /// A redirect is reported rather than followed. The Marketplace download
    /// endpoint answers 303 towards a separate file host, and deciding whether
    /// that host may be fetched from — and whether the credential travels with
    /// it — is policy, which belongs in [`crate::ContentApi`] next to the
    /// allow-lists rather than buried in a transport.
    fn download(
        &self,
        url: &str,
        authorization: Option<&str>,
        destination: &Path,
    ) -> Result<Download>;
}

/// The result of one download hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Download {
    /// Bytes written to the destination.
    Written(u64),
    /// The server answered 3xx with this `Location`.
    Redirect(String),
}
