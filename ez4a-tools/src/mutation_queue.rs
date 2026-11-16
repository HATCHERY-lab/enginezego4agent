// Portions derived from pi-durable 1.5.2 (packages/durable/src/tools/file-mutation-queue.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 tools/file-mutation-queue, MIT, see LICENSES/pi-MIT.txt.

//! In-memory queue of path mutations. No filesystem calls.
//!
//! Entries stay in push order. Two writes of one path both stay until
//! [`Queue::drain`]. `\` and `/` name the same path; `.` and `..` are not resolved.

/// Why [`Queue::push`] refused a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("path is empty")]
pub struct EmptyPath;

/// What a queued path mutation does. The queue does not apply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationKind {
    Write,
    Edit,
    Delete,
}

/// One path mutation, stored as given. Separators are not rewritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutation {
    pub path: String,
    pub kind: MutationKind,
}

/// Ordered mutations. Nothing here reads or writes the filesystem.
#[derive(Clone, Debug, Default)]
pub struct Queue {
    items: Vec<Mutation>,
}

impl Queue {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Append `mutation`. An empty path is rejected and the queue is unchanged.
    ///
    /// A second write of the same path is kept. Paths are not coalesced.
    pub fn push(&mut self, mutation: Mutation) -> Result<(), EmptyPath> {
        if mutation.path.is_empty() {
            return Err(EmptyPath);
        }
        self.items.push(mutation);
        Ok(())
    }

    /// Remove every mutation whose path is `path`, in push order.
    ///
    /// `\` and `/` match. Other paths stay, in their original order.
    pub fn drain(&mut self, path: &str) -> Vec<Mutation> {
        let key = normalize(path);
        let mut kept = Vec::with_capacity(self.items.len());
        let mut taken = Vec::new();
        for item in self.items.drain(..) {
            if normalize(&item.path) == key {
                taken.push(item);
            } else {
                kept.push(item);
            }
        }
        self.items = kept;
        taken
    }

    /// True when a queued path is `path`, a parent of it, or a child of it.
    ///
    /// Comparison uses `/` after treating `\` as `/`. A shared string prefix is
    /// not enough: `foo` does not overlap `foobar`.
    pub fn overlaps(&self, path: &str) -> bool {
        self.items.iter().any(|item| paths_overlap(&item.path, path))
    }
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let left = normalize(left);
    let right = normalize(right);
    if left == right {
        return true;
    }
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let (short, long) = if left.len() < right.len() {
        (left.as_str(), right.as_str())
    } else {
        (right.as_str(), left.as_str())
    };
    let Some(rest) = long.strip_prefix(short) else {
        return false;
    };
    short.ends_with('/') || rest.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutation(path: &str, kind: MutationKind) -> Mutation {
        Mutation {
            path: path.to_string(),
            kind,
        }
    }

    #[test]
    fn push_rejects_an_empty_path() {
        let mut queue = Queue::new();
        let error = queue
            .push(mutation("", MutationKind::Delete))
            .expect_err("empty path");
        assert_eq!(error, EmptyPath);
        assert!(queue.drain("").is_empty());
        assert!(!queue.overlaps(""));
        assert!(!queue.overlaps("/"));
    }

    #[test]
    fn two_writes_stay_until_drain_and_others_remain() {
        let mut queue = Queue::new();
        queue
            .push(mutation("a/b", MutationKind::Write))
            .expect("first write");
        queue
            .push(mutation("a/c", MutationKind::Delete))
            .expect("other");
        queue
            .push(mutation("a/b", MutationKind::Edit))
            .expect("edit");
        queue
            .push(mutation("a/b", MutationKind::Write))
            .expect("second write");
        queue
            .push(mutation("a/d", MutationKind::Write))
            .expect("tail");

        let drained = queue.drain("a/b");
        assert_eq!(
            drained,
            vec![
                mutation("a/b", MutationKind::Write),
                mutation("a/b", MutationKind::Edit),
                mutation("a/b", MutationKind::Write),
            ]
        );
        assert!(queue.drain("a/b").is_empty());
        assert_eq!(
            queue.drain("a/c"),
            vec![mutation("a/c", MutationKind::Delete)]
        );
        assert_eq!(
            queue.drain("a/d"),
            vec![mutation("a/d", MutationKind::Write)]
        );
    }

    #[test]
    fn drain_treats_backslash_as_slash_and_keeps_order() {
        let mut queue = Queue::new();
        queue
            .push(mutation(r"dir\file", MutationKind::Write))
            .expect("write");
        queue
            .push(mutation("dir/other", MutationKind::Edit))
            .expect("other");
        queue
            .push(mutation("dir/file", MutationKind::Delete))
            .expect("delete");

        let drained = queue.drain(r"dir\file");
        assert_eq!(
            drained,
            vec![
                mutation(r"dir\file", MutationKind::Write),
                mutation("dir/file", MutationKind::Delete),
            ]
        );
        assert!(!queue.overlaps("dir/file"));
        assert!(queue.overlaps("dir/other"));
        assert_eq!(
            queue.drain("dir/other"),
            vec![mutation("dir/other", MutationKind::Edit)]
        );
    }

    #[test]
    fn overlaps_self_parent_and_child_not_a_bare_prefix() {
        let mut queue = Queue::new();
        queue
            .push(mutation(r"src\lib.rs", MutationKind::Write))
            .expect("write");

        assert!(queue.overlaps("src/lib.rs"));
        assert!(queue.overlaps("src"));
        assert!(queue.overlaps("src/"));
        assert!(queue.overlaps(r"src\lib.rs\extra"));
        assert!(!queue.overlaps("src/lib.rs.bak"));
        assert!(!queue.overlaps("sr"));
        assert!(!queue.overlaps("other"));
        assert!(!queue.overlaps("src/lib"));
        assert!(!queue.overlaps(""));
    }
}
