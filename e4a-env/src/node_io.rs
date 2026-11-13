// Portions derived from pi-durable 1.5.2 (packages/durable/src/env/node.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 env/node, MIT, see LICENSES/pi-MIT.txt.

//! Checks for stat, mkdir, and rename. Nothing here calls the filesystem.
//!
//! `pi-durable` `env/node` uses `lstat`, `mkdir({ recursive: true })`, and
//! `fs.rename`. This slice only decides whether those operations are allowed.
//! The caller passes the stat result.

/// Kind reported for one path. A missing path is the `lstat` not-found case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Missing,
}

/// Whether renaming `from` onto `to` is allowed.
///
/// An empty path is invalid. The same path is refused, rather than treated as
/// a no-op. A missing source is not found. A directory source is refused:
/// this slice does not move trees. A file source is allowed. Destination
/// existence is not checked.
pub fn rename_ok(from: &str, to: &str, from_kind: FileKind) -> Result<(), String> {
    if from.is_empty() || to.is_empty() {
        return Err("path is empty".to_owned());
    }
    if from == to {
        return Err("rename source and destination are the same".to_owned());
    }
    match from_kind {
        FileKind::Missing => Err("source is missing".to_owned()),
        FileKind::Dir => Err("source is a directory".to_owned()),
        FileKind::File => Ok(()),
    }
}

/// Whether `mkdir` of `path` is allowed.
///
/// An empty path is invalid. `exists` means the directory is already there,
/// which Node's recursive `mkdir` accepts. A path that is not there yet is
/// also allowed. This does not create the directory.
pub fn mkdir_ok(path: &str, exists: bool) -> Result<(), String> {
    if path.is_empty() {
        return Err("path is empty".to_owned());
    }
    match exists {
        true => Ok(()),
        false => Ok(()),
    }
}

/// Join `rel` under `root` with `/`.
///
/// Rejects an empty relative path, an absolute one (`/`, `\`, or a drive
/// letter), and any `..` segment. `.` segments are kept. Does not resolve
/// the path on disk.
pub fn join_under(root: &str, rel: &str) -> Result<String, String> {
    if rel.is_empty() {
        return Err("path is empty".to_owned());
    }
    if is_absolute(rel) {
        return Err("path is absolute".to_owned());
    }
    if rel.split(['/', '\\']).any(|segment| segment == "..") {
        return Err("path escapes the root".to_owned());
    }
    Ok(join_slash(root, rel))
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with('\\')
        || matches!(path.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic())
}

fn join_slash(root: &str, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    if root.is_empty() {
        return rel;
    }
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    if root.is_empty() {
        format!("/{rel}")
    } else {
        format!("{root}/{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::{join_under, mkdir_ok, rename_ok, FileKind};

    #[test]
    fn rename_rejects_empty_same_missing_and_directory() {
        assert_eq!(rename_ok("", "b", FileKind::File), Err("path is empty".to_owned()));
        assert_eq!(rename_ok("a", "", FileKind::File), Err("path is empty".to_owned()));
        assert_eq!(
            rename_ok("a", "a", FileKind::File),
            Err("rename source and destination are the same".to_owned())
        );
        assert_eq!(
            rename_ok("a", "b", FileKind::Missing),
            Err("source is missing".to_owned())
        );
        assert_eq!(
            rename_ok("a", "b", FileKind::Dir),
            Err("source is a directory".to_owned())
        );
        assert_eq!(rename_ok("a", "b", FileKind::File), Ok(()));
        assert_eq!(rename_ok("a/b", "a\\b", FileKind::File), Ok(()));
    }

    #[test]
    fn mkdir_rejects_empty_and_accepts_existing() {
        assert_eq!(mkdir_ok("", false), Err("path is empty".to_owned()));
        assert_eq!(mkdir_ok("", true), Err("path is empty".to_owned()));
        assert_eq!(mkdir_ok("dir", true), Ok(()));
        assert_eq!(mkdir_ok("dir", false), Ok(()));
        assert_eq!(mkdir_ok("a/b", false), Ok(()));
    }

    #[test]
    fn join_under_rejects_escape_and_joins_with_slash() {
        assert_eq!(join_under("root", ""), Err("path is empty".to_owned()));
        assert_eq!(join_under("root", "/abs"), Err("path is absolute".to_owned()));
        assert_eq!(join_under("root", "\\abs"), Err("path is absolute".to_owned()));
        assert_eq!(join_under("root", "C:/win"), Err("path is absolute".to_owned()));
        assert_eq!(join_under("root", "c:rel"), Err("path is absolute".to_owned()));
        assert_eq!(join_under("root", ".."), Err("path escapes the root".to_owned()));
        assert_eq!(
            join_under("root", "a/../b"),
            Err("path escapes the root".to_owned())
        );
        assert_eq!(
            join_under("root", "a\\..\\b"),
            Err("path escapes the root".to_owned())
        );
        assert_eq!(join_under("root", "a/b"), Ok("root/a/b".to_owned()));
        assert_eq!(join_under("root/", "a\\b"), Ok("root/a/b".to_owned()));
        assert_eq!(join_under("C:\\work\\", "a\\b"), Ok("C:/work/a/b".to_owned()));
        assert_eq!(join_under("", "a\\b"), Ok("a/b".to_owned()));
        assert_eq!(join_under("/", "a"), Ok("/a".to_owned()));
        assert_eq!(join_under("root", "foo..bar"), Ok("root/foo..bar".to_owned()));
        assert_eq!(join_under("root", "a/./b"), Ok("root/a/./b".to_owned()));
    }
}
