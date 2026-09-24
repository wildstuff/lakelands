//! The one place a bundle's YAML is read.
//!
//! A document's text is read once into a [`Document`], and the strict
//! document types are deserialised from it. Every use of the YAML library
//! is in this file, so replacing it is a change to this file alone. (The
//! document stays in the library's own value type, not JSON, because that
//! keeps the order an author wrote fields in.)

use serde::de::DeserializeOwned;

/// One YAML document, read.
pub(super) struct Document(serde_yaml::Value);

impl Document {
    /// Read a document's text.
    pub(super) fn read(text: &str) -> Result<Document, String> {
        serde_yaml::from_str(text)
            .map(Document)
            .map_err(|e| e.to_string())
    }

    /// The document as one of the strict document types.
    pub(super) fn to<T: DeserializeOwned>(&self) -> Result<T, String> {
        serde_yaml::from_value(self.0.clone()).map_err(|e| e.to_string())
    }

    /// Its top-level `name`, if it has one that is text.
    pub(super) fn name(&self) -> Option<String> {
        self.0
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_owned)
    }
}
