// engine4agent task graph, not a verbatim upstream file.

//! Parent links among tasks.
//!
//! Direct children and roots keep input order. No store, process, or other I/O.

/// One task and the id of its parent. `None` is a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskNode {
    pub id: i64,
    pub parent: Option<i64>,
}

/// Ids of nodes whose parent is `id`, in input order.
pub fn children(nodes: &[TaskNode], id: i64) -> Vec<i64> {
    nodes
        .iter()
        .filter(|node| node.parent == Some(id))
        .map(|node| node.id)
        .collect()
}

/// Whether setting `id`'s parent to `new_parent` would cycle.
///
/// `None` is never a cycle. A parent equal to `id` is a cycle. Existing parents
/// are walked from `new_parent` and must not reach `id`. A parent id missing
/// from `nodes` is not a cycle. The walk stops after `nodes.len() + 1` steps;
/// a longer walk is a cycle. The first node with a given id supplies its parent.
pub fn would_cycle(nodes: &[TaskNode], id: i64, new_parent: Option<i64>) -> bool {
    let Some(mut current) = new_parent else {
        return false;
    };
    for _ in 0..nodes.len() + 1 {
        if current == id {
            return true;
        }
        let Some(node) = nodes.iter().find(|node| node.id == current) else {
            return false;
        };
        match node.parent {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    true
}

/// Ids of nodes whose parent is `None`, in input order.
pub fn roots(nodes: &[TaskNode]) -> Vec<i64> {
    nodes
        .iter()
        .filter(|node| node.parent.is_none())
        .map(|node| node.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, parent: Option<i64>) -> TaskNode {
        TaskNode { id, parent }
    }

    #[test]
    fn children_are_direct_and_keep_input_order() {
        let nodes = [
            node(3, Some(1)),
            node(1, None),
            node(2, Some(1)),
            node(4, Some(3)),
            node(5, Some(9)),
            node(1, Some(1)),
        ];
        assert_eq!(children(&nodes, 1), vec![3, 2, 1]);
        assert_eq!(children(&nodes, 3), vec![4]);
        assert_eq!(children(&nodes, 4), Vec::<i64>::new());
        assert_eq!(children(&nodes, 9), vec![5]);
        assert!(children(&[], 1).is_empty());
    }

    #[test]
    fn roots_are_parentless_in_input_order() {
        let nodes = [
            node(2, Some(1)),
            node(1, None),
            node(4, Some(99)),
            node(3, None),
        ];
        assert_eq!(roots(&nodes), vec![1, 3]);
        assert!(roots(&[]).is_empty());
    }

    #[test]
    fn none_self_ancestor_missing_and_capped_walk() {
        let chain = [node(1, None), node(2, Some(1)), node(3, Some(2))];
        assert!(!would_cycle(&chain, 3, None));
        assert!(would_cycle(&chain, 3, Some(3)));
        assert!(would_cycle(&chain, 1, Some(3)));
        assert!(would_cycle(&chain, 2, Some(3)));
        assert!(!would_cycle(&chain, 3, Some(1)));
        assert!(!would_cycle(&chain, 4, Some(3)));
        assert!(!would_cycle(&chain, 1, Some(99)));
        assert!(!would_cycle(&[], 1, Some(2)));
        assert!(would_cycle(&[], 1, Some(1)));

        let looped = [node(1, Some(2)), node(2, Some(1))];
        assert!(would_cycle(&looped, 3, Some(1)));

        let dangling = [node(1, Some(50))];
        assert!(!would_cycle(&dangling, 9, Some(1)));

        let first_parent = [node(7, None), node(7, Some(8)), node(8, Some(7))];
        assert!(!would_cycle(&first_parent, 8, Some(7)));
    }
}
