//! Dependency-inversion seam for the pluggable value kinds whose mxrb
//! behavior delegates to `Forms::MprCodec#decode_embedded` (`TextTemplate`,
//! `Action`, `Icon`, `DataSource`, and native `Widgets`). `mxrs-forms`
//! already depends on `mxrs-pluggable` (to decode a `CustomWidgets$
//! CustomWidget` nested inside an ordinary page); if this crate depended
//! back on `mxrs-forms` to decode those kinds, the workspace would
//! have a dependency cycle. mxrb avoids the equivalent problem by holding
//! a `forms_codec:` callback object (see `Pluggable::MprCodec#initialize`);
//! this trait is that same seam expressed as a Rust trait instead of a
//! duck-typed reference, so `mxrs-pluggable` names only the *shape* of an
//! embedded-Forms decoder, never the concrete `mxrs-forms` type.
use mxrs_bson::Document;

/// Decodes an arbitrary embedded Forms element (any element `Node`
/// `mxrs-forms`'s own `Catalog` can resolve) into `Self::Node`.
/// `mxrs-forms::MprCodec`
/// implements this once, closing over whatever `local_references` map is
/// live for the surrounding top-level decode — mirroring mxrb's own
/// comment on `decode_embedded`: "Nested pluggable values share the
/// root's semantic reference maps."
pub trait EmbeddedFormsDecoder {
    type Node;
    type Error: std::error::Error + Send + Sync + 'static;

    fn decode_embedded(
        &self,
        document: &Document,
        path: &str,
    ) -> std::result::Result<Self::Node, Self::Error>;
}

/// Encoder-side counterpart to [`EmbeddedFormsDecoder`]. Keeping this as
/// a separate trait lets read-only integrations continue to provide only a
/// decoder, while the full `mxrs-forms` codec implements both directions.
pub trait EmbeddedFormsEncoder {
    type Node;
    type Error: std::error::Error + Send + Sync + 'static;

    fn encode_embedded(
        &self,
        node: &Self::Node,
        path: &str,
    ) -> std::result::Result<Document, Self::Error>;
}
