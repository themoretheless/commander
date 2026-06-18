//! Bookmarks extracted (SRP: favorites/hotlist separate).
//! Small piece for understanding. Includes persist ready.

use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
}
