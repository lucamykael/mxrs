//! Image collections: the images a module's pages show, each its bytes and
//! the format they are in.

use crate::ExportLevel;

/// The format an image's bytes are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpg,
    Gif,
    Bmp,
    Svg,
}

impl ImageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Png => "Png",
            ImageFormat::Jpg => "Jpg",
            ImageFormat::Gif => "Gif",
            ImageFormat::Bmp => "Bmp",
            ImageFormat::Svg => "Svg",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Png" => Self::Png,
            "Jpg" => Self::Jpg,
            "Gif" => Self::Gif,
            "Bmp" => Self::Bmp,
            "Svg" => Self::Svg,
            _ => return None,
        })
    }

    /// The extension a file of the format has.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpg => "jpg",
            ImageFormat::Gif => "gif",
            ImageFormat::Bmp => "bmp",
            ImageFormat::Svg => "svg",
        }
    }
}

/// One image of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDecl {
    pub name: String,
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

/// An image collection of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCollectionDecl {
    pub name: String,
    pub documentation: String,
    pub images: Vec<ImageDecl>,
    pub excluded: bool,
    pub export_level: ExportLevel,
}

impl ImageCollectionDecl {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            documentation: String::new(),
            images: Vec::new(),
            excluded: false,
            export_level: ExportLevel::Hidden,
        }
    }
}
