//! The structure hierarchy (`brainglobe_atlasapi.structure_tree_util`).
//!
//! [`Tree`] is the part of `treelib.Tree` (1.8) the API exposes: nodes with
//! a tag and an id, children in insertion order, `leaves()` and `show()`
//! with treelib's ordering (children sorted by tag) and its `├──` drawing.

use std::collections::{HashMap, VecDeque};

use crate::error::{Error, Result};
use crate::structure_class::StructureRecord;

/// A node of the tree (`treelib.Node`).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub tag: String,
    pub identifier: i64,
    parent: Option<i64>,
    children: Vec<i64>,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
    pub fn predecessor(&self) -> Option<i64> {
        self.parent
    }
    pub fn successors(&self) -> &[i64] {
        &self.children
    }
}

/// A tree of structures (`treelib.Tree`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    nodes: Vec<Node>,
    index: HashMap<i64, usize>,
    root: Option<i64>,
}

impl Tree {
    pub fn new() -> Tree {
        Tree::default()
    }

    /// `create_node(tag, identifier, parent)`.
    pub fn create_node(&mut self, tag: &str, identifier: i64, parent: Option<i64>) -> Result<()> {
        if self.index.contains_key(&identifier) {
            return Err(Error::value(format!(
                "Can't create node with ID '{identifier}'"
            )));
        }
        match parent {
            None => {
                if self.root.is_some() {
                    return Err(Error::value("A tree takes one root merely.".to_string()));
                }
                self.root = Some(identifier);
            }
            Some(p) => {
                let pi = *self
                    .index
                    .get(&p)
                    .ok_or_else(|| Error::value(format!("Parent node '{p}' is not in the tree")))?;
                self.nodes[pi].children.push(identifier);
            }
        }
        self.index.insert(identifier, self.nodes.len());
        self.nodes.push(Node {
            tag: tag.to_string(),
            identifier,
            parent,
            children: vec![],
        });
        Ok(())
    }

    pub fn root(&self) -> Option<i64> {
        self.root
    }

    /// `tree[nid]` / `get_node(nid)`.
    pub fn get_node(&self, nid: i64) -> Option<&Node> {
        self.index.get(&nid).map(|&i| &self.nodes[i])
    }

    pub fn contains(&self, nid: i64) -> bool {
        self.index.contains_key(&nid)
    }

    /// `tree.nodes`, in insertion order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    fn node(&self, nid: i64) -> Result<&Node> {
        self.get_node(nid)
            .ok_or_else(|| Error::value(format!("Node '{nid}' is not in the tree")))
    }

    /// `children(nid)`, in insertion order.
    pub fn children(&self, nid: i64) -> Result<Vec<&Node>> {
        let n = self.node(nid)?;
        Ok(n.children
            .iter()
            .map(|c| &self.nodes[self.index[c]])
            .collect())
    }

    fn sorted_children(&self, nid: i64) -> Vec<&Node> {
        let mut c: Vec<&Node> = self.nodes[self.index[&nid]]
            .children
            .iter()
            .map(|c| &self.nodes[self.index[c]])
            .collect();
        // a stable sort by tag, as `list.sort` on treelib nodes
        c.sort_by(|a, b| a.tag.cmp(&b.tag));
        c
    }

    /// `expand_tree(nid)` (depth first, children sorted by tag).
    pub fn expand_tree(&self, nid: Option<i64>) -> Result<Vec<i64>> {
        let nid = match nid.or(self.root) {
            Some(n) => n,
            None => return Ok(vec![]),
        };
        if !self.contains(nid) {
            return Err(Error::value(format!("Node '{nid}' is not in the tree")));
        }
        let mut out = vec![nid];
        let mut queue: VecDeque<&Node> = self.sorted_children(nid).into();
        while let Some(n) = queue.pop_front() {
            out.push(n.identifier);
            for c in self.sorted_children(n.identifier).into_iter().rev() {
                queue.push_front(c);
            }
        }
        Ok(out)
    }

    /// `leaves(nid)`: with no `nid`, every leaf in insertion order; else the
    /// leaves under `nid` in [`expand_tree`](Tree::expand_tree) order.
    pub fn leaves(&self, nid: Option<i64>) -> Result<Vec<&Node>> {
        match nid {
            None => Ok(self.nodes.iter().filter(|n| n.is_leaf()).collect()),
            Some(nid) => Ok(self
                .expand_tree(Some(nid))?
                .into_iter()
                .map(|i| &self.nodes[self.index[&i]])
                .filter(|n| n.is_leaf())
                .collect()),
        }
    }

    /// `show(stdout=False)`: the tree drawn with `├──` / `└──`, one line per
    /// node (each ending in a newline), children sorted by tag.
    pub fn show(&self) -> String {
        let Some(root) = self.root else {
            // treelib prints this and returns the empty reader
            return String::new();
        };
        let mut out = String::new();
        let mut is_last: Vec<bool> = vec![];
        self.show_iter(root, true, &mut is_last, &mut out);
        out
    }

    fn show_iter(&self, nid: i64, is_root: bool, is_last: &mut Vec<bool>, out: &mut String) {
        let node = &self.nodes[self.index[&nid]];
        if !is_root {
            for &last in &is_last[..is_last.len() - 1] {
                out.push_str(if last { "    " } else { "\u{2502}   " });
            }
            out.push_str(if *is_last.last().unwrap() {
                "\u{2514}\u{2500}\u{2500} "
            } else {
                "\u{251c}\u{2500}\u{2500} "
            });
        }
        out.push_str(&node.tag);
        out.push('\n');
        let children = self.sorted_children(nid);
        let last = children.len().saturating_sub(1);
        for (i, c) in children.iter().enumerate() {
            is_last.push(i == last);
            self.show_iter(c.identifier, false, is_last, out);
            is_last.pop();
        }
    }
}

impl std::fmt::Display for Tree {
    /// `str(tree)`, which is what `print(tree)` shows.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.show())
    }
}

/// The ids of the children of `structure` (`child_ids`).
pub fn child_ids(structure: i64, structure_list: &[StructureRecord]) -> Vec<i64> {
    structure_list
        .iter()
        .filter(|s| {
            let p = s.path();
            p.len() > 1 && p[p.len() - 2] as i64 == structure
        })
        .map(|s| s.id)
        .collect()
}

/// The hierarchy of all structures as a [`Tree`] (`get_structures_tree`).
pub fn get_structures_tree(structures_list: &[StructureRecord]) -> Result<Tree> {
    let acronym = |id: i64| -> String {
        structures_list
            .iter()
            .rev()
            .find(|s| s.id == id)
            .map(|s| s.acronym_str())
            .unwrap_or_default()
    };
    // `{v: k for k, v in id_to_acronym_map.items()}["root"]`: the last
    // structure called root wins
    let mut id_to_acronym: Vec<(i64, String)> = vec![];
    for s in structures_list {
        match id_to_acronym.iter_mut().find(|(i, _)| *i == s.id) {
            Some(e) => e.1 = s.acronym_str(),
            None => id_to_acronym.push((s.id, s.acronym_str())),
        }
    }
    let root = id_to_acronym
        .iter()
        .rev()
        .find(|(_, a)| a == "root")
        .map(|(i, _)| *i)
        .ok_or_else(|| Error::key("root"))?;

    // children of every id, in list order, computed once (child_ids is a
    // scan of the whole list per node in Python)
    let mut kids: HashMap<i64, Vec<i64>> = HashMap::new();
    for s in structures_list {
        let p = s.path();
        if p.len() > 1 {
            kids.entry(p[p.len() - 2] as i64).or_default().push(s.id);
        }
    }

    let mut tree = Tree::new();
    tree.create_node(&format!("root ({root})"), root, None)?;
    // depth first, as the recursion in Python
    let mut stack: Vec<(i64, i64)> = kids
        .get(&root)
        .map(|c| c.iter().rev().map(|&c| (c, root)).collect())
        .unwrap_or_default();
    while let Some((id, parent)) = stack.pop() {
        tree.create_node(&format!("{} ({id})", acronym(id)), id, Some(parent))?;
        if let Some(c) = kids.get(&id) {
            stack.extend(c.iter().rev().map(|&c| (c, id)));
        }
    }
    Ok(tree)
}

/// Nodes in pre-order depth first order (`preorder_depth_first_search`):
/// children in insertion order.
pub fn preorder_depth_first_search(tree: &Tree) -> Vec<&Node> {
    let Some(root) = tree.root() else {
        return vec![];
    };
    let mut out = vec![];
    let mut stack = vec![tree.get_node(root).unwrap()];
    while let Some(n) = stack.pop() {
        out.push(n);
        for c in n.children.iter().rev() {
            stack.push(tree.get_node(*c).unwrap());
        }
    }
    out
}

/// Nodes in pre-order breadth first order (`preorder_breadth_first_search`).
pub fn preorder_breadth_first_search(tree: &Tree) -> Vec<&Node> {
    let Some(root) = tree.root() else {
        return vec![];
    };
    let mut out = vec![];
    let mut queue: VecDeque<&Node> = VecDeque::from([tree.get_node(root).unwrap()]);
    while let Some(n) = queue.pop_front() {
        out.push(n);
        for c in &n.children {
            queue.push_back(tree.get_node(*c).unwrap());
        }
    }
    out
}

/// Nodes in post-order depth first order, leaves first
/// (`postorder_depth_first_search`).
pub fn postorder_depth_first_search(tree: &Tree) -> Vec<&Node> {
    let Some(root) = tree.root() else {
        return vec![];
    };
    let mut out = vec![];
    // (node, children visited)
    let mut stack: Vec<(i64, bool)> = vec![(root, false)];
    while let Some((id, done)) = stack.pop() {
        if done {
            out.push(tree.get_node(id).unwrap());
            continue;
        }
        stack.push((id, true));
        for c in tree.get_node(id).unwrap().children.iter().rev() {
            stack.push((*c, false));
        }
    }
    out
}
