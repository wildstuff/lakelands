//! The one place a bundle's YAML is read.
//!
//! A document's text is checked once when it is read, and then read to
//! one value the document holds: `name` reads it, and every document
//! type is read from it through `strict`, so a struct written as a
//! sequence is refused with the contract's sentence. Every use of the
//! YAML library is in this file, so replacing it is a change to this
//! file alone. The library is `serde-saphyr`, a maintained, pure-Rust
//! YAML 1.2 reader.

use serde::de::{DeserializeOwned, IgnoredAny};

/// One YAML document, read.
pub(super) struct Document(serde_json::Value);

impl Document {
    /// Read a document's text: it must be one well-formed YAML document.
    pub(super) fn read(text: &str) -> Result<Document, String> {
        serde_saphyr::from_str::<IgnoredAny>(text).map_err(|e| e.to_string())?;
        let doc: serde_json::Value = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
        Ok(Document(doc))
    }

    /// The document as one of the strict document types.
    pub(super) fn to<T: DeserializeOwned>(&self) -> Result<T, String> {
        lakelands_api::strict::from_value(self.0.clone()).map_err(|e| e.to_string())
    }

    /// Its top-level `name`, if it has one that is text.
    pub(super) fn name(&self) -> Option<String> {
        self.0.get("name")?.as_str().map(str::to_owned)
    }
}
