//! Open the harness SQLite file. Does not run a model and does not apply
//! [`crate::OpenOptions`].

use std::path::Path;

/// Open `path` with [`e4a_store::SqliteStorage::open`].
///
/// An empty path is rejected before open, so no file is created.
pub fn open_sqlite(path: &Path) -> Result<e4a_store::SqliteStorage, e4a_store::Error> {
    if path.as_os_str().is_empty() {
        return Err(e4a_store::Error::message("path is empty"));
    }
    e4a_store::SqliteStorage::open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_empty_path_without_opening() {
        match open_sqlite(Path::new("")) {
            Ok(_) => panic!("empty path must be rejected"),
            Err(error) => assert_eq!(error, e4a_store::Error::message("path is empty")),
        }
    }
}
