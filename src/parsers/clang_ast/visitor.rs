//! Visitor pattern for walking [`AstNode`] trees.
//!
//! This replaces the ad-hoc `fn(&Value, &mut State) { recurse into inner }`
//! pattern that was repeated dozens of times across the original module.
//! Visitors implement [`Visitor::enter`] (called pre-order on every node)
//! and optionally [`Visitor::leave`] (called post-order, useful for
//! maintaining an enclosing-context stack).
//!
//! Returning [`WalkAction::SkipChildren`] from `enter` prunes the subtree.

use super::node::AstNode;

/// Controls whether [`walk`] descends into a node's children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkAction {
    /// Recurse into `node.inner` as normal.
    Continue,
    /// Skip this subtree but keep walking siblings.
    SkipChildren,
}

/// Pre/post-order tree walker callbacks.
pub trait Visitor {
    /// Called once on every node in pre-order traversal. Default behavior
    /// is to descend into all children.
    fn enter(&mut self, _node: &AstNode) -> WalkAction {
        WalkAction::Continue
    }

    /// Called once on every node in post-order traversal. Default is a
    /// no-op. Useful for popping a context stack.
    fn leave(&mut self, _node: &AstNode) {}
}

/// Walk `root` and every transitive child, invoking `visitor` in pre/post
/// order. Recursion depth is bounded by the AST's own depth.
pub fn walk<V: Visitor>(root: &AstNode, visitor: &mut V) {
    let action = visitor.enter(root);
    if action == WalkAction::Continue {
        for child in &root.inner {
            walk(child, visitor);
        }
    }
    visitor.leave(root);
}

/// Convenience helper for visitors that need to know the *enclosing class*
/// of a node — a recurring need across this crate.  Maintains a stack of
/// `CXXRecordDecl` / `ClassTemplateSpecializationDecl` names and exposes
/// the innermost one as `current()`.
#[derive(Debug, Default, Clone)]
pub struct ClassStack {
    names: Vec<String>,
}

impl ClassStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push the class name from a node iff it defines a class layout.
    /// Returns `true` if a push happened so callers can pair it with a
    /// matching `pop_if(true)` from their `leave` callback.
    pub fn push_if_class(&mut self, node: &AstNode) -> bool {
        if let Some(name) = node.class_name() {
            self.names.push(name.to_string());
            true
        } else {
            false
        }
    }

    /// Pop the top entry when `pushed` is true. Pair with `push_if_class`.
    pub fn pop_if(&mut self, pushed: bool) {
        if pushed {
            self.names.pop();
        }
    }

    /// Innermost enclosing class, if any.
    pub fn current(&self) -> Option<&str> {
        self.names.last().map(|s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> AstNode {
        serde_json::from_value(v).unwrap()
    }

    /// Counts every node in the tree.
    struct Counter(usize);
    impl Visitor for Counter {
        fn enter(&mut self, _: &AstNode) -> WalkAction {
            self.0 += 1;
            WalkAction::Continue
        }
    }

    #[test]
    fn walk_visits_every_node_pre_order() {
        let n = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "name": "a", "inner": [
                    {"kind": "ParmVarDecl", "name": "x"}
                ]},
                {"kind": "FunctionDecl", "name": "b"}
            ]
        }));
        let mut c = Counter(0);
        walk(&n, &mut c);
        assert_eq!(c.0, 4); // TU + 2 funcs + 1 parm
    }

    /// Collects the names of every CXXRecordDecl + records the enclosing
    /// class for every CXXMethodDecl.
    struct Collector {
        stack: ClassStack,
        method_classes: Vec<(String, Option<String>)>,
    }
    impl Visitor for Collector {
        fn enter(&mut self, node: &AstNode) -> WalkAction {
            if node.kind == "CXXMethodDecl" {
                self.method_classes.push((
                    node.name.clone().unwrap_or_default(),
                    self.stack.current().map(String::from),
                ));
            }
            self.stack.push_if_class(node);
            WalkAction::Continue
        }
        fn leave(&mut self, node: &AstNode) {
            self.stack.pop_if(node.class_name().is_some());
        }
    }

    #[test]
    fn class_stack_tracks_enclosing_class_through_nesting() {
        let n = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {
                    "kind": "CXXRecordDecl",
                    "name": "Outer",
                    "inner": [
                        {"kind": "CXXMethodDecl", "name": "method_in_outer"},
                        {
                            "kind": "CXXRecordDecl",
                            "name": "Inner",
                            "inner": [
                                {"kind": "CXXMethodDecl", "name": "method_in_inner"}
                            ]
                        }
                    ]
                },
                {"kind": "CXXMethodDecl", "name": "top_level_method"}
            ]
        }));
        let mut c = Collector {
            stack: ClassStack::new(),
            method_classes: Vec::new(),
        };
        walk(&n, &mut c);
        assert_eq!(
            c.method_classes,
            vec![
                ("method_in_outer".into(), Some("Outer".into())),
                ("method_in_inner".into(), Some("Inner".into())),
                ("top_level_method".into(), None),
            ]
        );
    }

    /// Verifies SkipChildren prunes a subtree.
    struct PruneFunctionDecls(usize);
    impl Visitor for PruneFunctionDecls {
        fn enter(&mut self, node: &AstNode) -> WalkAction {
            self.0 += 1;
            if node.kind == "FunctionDecl" {
                WalkAction::SkipChildren
            } else {
                WalkAction::Continue
            }
        }
    }

    #[test]
    fn skip_children_prunes_subtree() {
        let n = parse(json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                {"kind": "FunctionDecl", "inner": [
                    {"kind": "ParmVarDecl"},
                    {"kind": "ParmVarDecl"}
                ]},
                {"kind": "VarDecl"}
            ]
        }));
        let mut v = PruneFunctionDecls(0);
        walk(&n, &mut v);
        // TU + FunctionDecl (skipped) + VarDecl == 3 visits;
        // the two ParmVarDecl children are pruned.
        assert_eq!(v.0, 3);
    }

    #[test]
    fn leave_runs_in_post_order() {
        struct Order(Vec<(String, &'static str)>);
        impl Visitor for Order {
            fn enter(&mut self, n: &AstNode) -> WalkAction {
                self.0.push((n.kind.clone(), "enter"));
                WalkAction::Continue
            }
            fn leave(&mut self, n: &AstNode) {
                self.0.push((n.kind.clone(), "leave"));
            }
        }
        let n = parse(json!({
            "kind": "A",
            "inner": [{"kind": "B"}]
        }));
        let mut o = Order(Vec::new());
        walk(&n, &mut o);
        assert_eq!(
            o.0,
            vec![
                ("A".into(), "enter"),
                ("B".into(), "enter"),
                ("B".into(), "leave"),
                ("A".into(), "leave"),
            ]
        );
    }
}
