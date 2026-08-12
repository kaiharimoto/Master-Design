//! Stable identifiers.
//!
//! Every node carries an id that survives moves, renames and round-trips through the
//! AI. Ids are the anchor the patch protocol addresses, so they must never be reused
//! and never be positional.
//!
//! The `nd_` / `pg_` / `tl_` prefixes are for humans and for models: when a selector or
//! an error message mentions `nd_01j...`, it is obvious what kind of thing it refers to
//! without looking it up.

use crate::error::{DocError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! prefixed_id {
    ($name:ident, $prefix:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// Mint a fresh id.
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                $name(format!(
                    "{}{}",
                    $prefix,
                    ulid::Ulid::new().to_string().to_lowercase()
                ))
            }

            /// Accept an existing id, checking the prefix.
            pub fn parse(s: impl Into<String>) -> Result<Self> {
                let s = s.into();
                if !s.starts_with($prefix) || s.len() <= $prefix.len() {
                    return Err(DocError::InvalidId(s));
                }
                if !s[$prefix.len()..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return Err(DocError::InvalidId(s));
                }
                Ok($name(s))
            }

            /// Build an id without validation.
            ///
            /// For tests and fixtures that need predictable, readable ids. Anything
            /// reading a file or accepting input from the AI should use
            /// [`Self::parse`] instead.
            pub fn from_static(s: &'static str) -> Self {
                $name(s.to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

prefixed_id!(
    NodeId,
    "nd_",
    "Identifier for a node in a page's scene graph."
);
prefixed_id!(PageId, "pg_", "Identifier for a page.");
prefixed_id!(TimelineId, "tl_", "Identifier for a timeline.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ids_are_unique_and_prefixed() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("nd_"));
    }

    #[test]
    fn parse_rejects_wrong_prefix_and_junk() {
        assert!(NodeId::parse("pg_01").is_err());
        assert!(NodeId::parse("nd_").is_err());
        assert!(NodeId::parse("nd_has spaces").is_err());
        assert!(NodeId::parse("nd_01hq5").is_ok());
    }

    #[test]
    fn ids_round_trip_through_json_as_bare_strings() {
        let id = NodeId::from_static("nd_hero");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"nd_hero\"");
    }
}
