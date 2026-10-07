//! An image collection, as a module declares it:
//!
//! ```
//! # use mxrs_dsl::ModuleBuilder;
//! # use mxrs_ir::ImageFormat;
//! # let mut module = ModuleBuilder::new("Sales");
//! module.image_collection("Images", |images| {
//!     images.image("logo", ImageFormat::Svg, b"<svg/>");
//! });
//! ```

use mxrs_ir::{ExportLevel, ImageCollectionDecl, ImageDecl, ImageFormat};

pub struct ImageCollectionBuilder {
    decl: ImageCollectionDecl,
}

impl ImageCollectionBuilder {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            decl: ImageCollectionDecl::new(name),
        }
    }

    /// An image: its name, its format and its bytes.
    pub fn image(
        &mut self,
        name: impl Into<String>,
        format: ImageFormat,
        data: &[u8],
    ) -> &mut Self {
        self.decl.images.push(ImageDecl {
            name: name.into(),
            format,
            data: data.to_vec(),
        });
        self
    }

    pub fn documentation(&mut self, value: impl Into<String>) -> &mut Self {
        self.decl.documentation = value.into();
        self
    }

    pub fn excluded(&mut self, value: bool) -> &mut Self {
        self.decl.excluded = value;
        self
    }

    pub fn export_level(&mut self, value: ExportLevel) -> &mut Self {
        self.decl.export_level = value;
        self
    }

    pub(crate) fn into_decl(self) -> ImageCollectionDecl {
        self.decl
    }
}
