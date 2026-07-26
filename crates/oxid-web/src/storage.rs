//! Links kept in the browser, so closing the tab does not lose them.
//!
//! `localStorage`, not a cookie. A cookie for `oxid.uk` would be attached to
//! every request to the origin — including each `/{code}` redirect, the one path
//! the whole system is tuned around. This list never touches the network.
//!
//! Survives closing the tab and the browser. It does not survive clearing site
//! data, another profile, or another device; stage 11 is what makes the list
//! follow an account instead of a browser.

use gloo_storage::{LocalStorage, Storage, errors::StorageError};
use leptos::logging;
use oxid_shared::ShortenResponse;
use serde::{Deserialize, Serialize};

/// Versioned on purpose: a future shape change reads a different key instead of
/// deserializing old entries into a struct that no longer matches.
const KEY: &str = "oxid.links.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLink {
    pub code: String,
    pub short_url: String,
    pub long_url: String,
}

impl From<ShortenResponse> for SavedLink {
    fn from(response: ShortenResponse) -> Self {
        Self {
            code: response.code,
            short_url: response.short_url,
            long_url: response.long_url,
        }
    }
}

impl SavedLink {
    /// Characters this link removed. Saturating because a "short" URL longer
    /// than its target is possible in development, where the origin is
    /// `http://localhost:8080` and the target may be a two-character path.
    pub fn saved_chars(&self) -> usize {
        self.long_url
            .chars()
            .count()
            .saturating_sub(self.short_url.chars().count())
    }
}

/// Reads the list, treating anything unreadable as empty.
///
/// A corrupt entry must not stop the page from loading, and the next write
/// fixes it. The reason is still logged — silence here would hide a
/// serialization bug behind "the list is empty".
pub fn load() -> Vec<SavedLink> {
    match LocalStorage::get::<Vec<SavedLink>>(KEY) {
        Ok(links) => links,
        Err(StorageError::KeyNotFound(_)) => Vec::new(),
        Err(error) => {
            logging::warn!("discarding unreadable saved links: {error}");
            Vec::new()
        }
    }
}

/// Writes the list, handing failure back to the caller.
///
/// Storage can genuinely fail — Safari private mode and a full quota both
/// reject writes — so the caller decides what to tell the person. Losing the
/// list silently is the one outcome this feature exists to prevent.
pub fn save(links: &[SavedLink]) -> Result<(), StorageError> {
    LocalStorage::set(KEY, links)
}

/// Puts a link at the top, and never twice.
///
/// The API is idempotent, so shortening the same URL again returns the same
/// code — without the dedupe the list would fill with copies of whatever the
/// person is pasting repeatedly.
pub fn prepend(links: &mut Vec<SavedLink>, link: SavedLink) {
    links.retain(|saved| saved.code != link.code);
    links.insert(0, link);
}
