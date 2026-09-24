//! The one place a bundle's YAML is read.
//!
//! A document's text is checked once when it is read, and the strict
//! document types are deserialised from that text. Every use of the YAML
//! library is in this file, so replacing it is a change to this file
//! alone. The library is `serde-saphyr`, a maintained, pure-Rust YAML 1.2
//! reader. Deserialising each document type straight from the text keeps
//! the order an author wrote fields in.

use serde::de::{DeserializeOwned, IgnoredAny};

/// One YAML document, read.
pub(super) struct Document(String);

impl Document {
    /// Read a document's text: it must be one well-formed YAML document.
    pub(super) fn read(text: &str) -> Result<Document, String> {
        serde_saphyr::from_str::<IgnoredAny>(text).map_err(|e| e.to_string())?;
        Ok(Document(text.to_owned()))
    }

    /// The document as one of the strict document types.
    pub(super) fn to<T: DeserializeOwned>(&self) -> Result<T, String> {
        serde_saphyr::from_str(&self.0).map_err(|e| e.to_string())
    }

    /// Its top-level `name`, if it has one that is text.
    pub(super) fn name(&self) -> Option<String> {
        let doc: serde_json::Value = serde_saphyr::from_str(&self.0).ok()?;
        doc.get("name")?.as_str().map(str::to_owned)
    }
}
