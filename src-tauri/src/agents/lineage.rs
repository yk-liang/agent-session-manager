//! 子会话关系。Adapter 只登记会话位置与数据源记录的父会话；
//! 父会话是否可用、孤儿、项目归属和级联删除范围统一在这里判定。
use super::{SessionLocation, Subagent};
use std::collections::{BTreeMap, HashSet};

pub(crate) struct Node {
    pub id: String,
    /// 可读取的会话位置；仅出现在关系索引里的会话没有位置，不能作为可打开的父会话。
    pub location: Option<SessionLocation>,
    /// 数据源记录的父会话 ID；多于一个表示记录互相冲突。
    pub parents: Vec<String>,
    /// 数据源把它标为子会话，即使没有记录父会话。
    pub child: bool,
}

impl Node {
    pub fn new(location: SessionLocation, parent: Option<String>) -> Self {
        Self { id: location.session_id.clone(), child: parent.is_some(), parents: parent.into_iter().collect(), location: Some(location) }
    }
}

pub(crate) struct Lineage {
    nodes: BTreeMap<String, Node>,
    /// 成环的会话互为祖先，父关系一律不可信。
    cycles: HashSet<String>,
}

impl Lineage {
    pub fn new(nodes: impl IntoIterator<Item = Node>) -> Self {
        let mut merged = BTreeMap::<String, Node>::new();
        for node in nodes {
            match merged.get_mut(&node.id) {
                Some(known) => {
                    known.child |= node.child;
                    known.location = known.location.take().or(node.location);
                    known.parents.extend(node.parents);
                }
                None => { merged.insert(node.id.clone(), node); }
            }
        }
        for node in merged.values_mut() {
            node.parents.sort_unstable();
            node.parents.dedup();
        }
        let mut lineage = Self { nodes: merged, cycles: HashSet::new() };
        lineage.cycles = lineage.find_cycles();
        lineage
    }

    fn find_cycles(&self) -> HashSet<String> {
        let mut cycles = HashSet::new();
        let mut settled = HashSet::new();
        for id in self.nodes.keys() {
            let mut path = Vec::new();
            let mut current = Some(id.as_str());
            while let Some(id) = current.filter(|id| !settled.contains(*id)) {
                if let Some(start) = path.iter().position(|seen| *seen == id) {
                    cycles.extend(path[start..].iter().map(|id: &&str| (*id).to_owned()));
                    break;
                }
                path.push(id);
                current = self.recorded_parent(id);
            }
            settled.extend(path);
        }
        cycles
    }

    fn recorded_parent(&self, id: &str) -> Option<&str> {
        match self.nodes.get(id)?.parents.as_slice() {
            [parent] => Some(parent),
            _ => None,
        }
    }

    pub fn location(&self, id: &str) -> Option<&SessionLocation> {
        self.nodes.get(id)?.location.as_ref()
    }

    fn parent(&self, id: &str) -> Option<&SessionLocation> {
        if self.cycles.contains(id) {
            return None;
        }
        self.location(self.recorded_parent(id)?)
    }

    pub fn is_child(&self, id: &str) -> bool {
        self.nodes.get(id).is_some_and(|node| node.child || !node.parents.is_empty())
    }

    /// 父会话缺失、记录冲突或成环时标为孤儿；项目归属取可追溯的最上层祖先。
    pub fn subagent(&self, id: &str) -> Option<Subagent> {
        if !self.is_child(id) {
            return None;
        }
        let parent = self.parent(id);
        let mut owner = parent;
        while let Some(next) = owner.and_then(|owner| self.parent(&owner.session_id)) {
            owner = Some(next);
        }
        Some(Subagent {
            id: None,
            parent_id: self.recorded_parent(id).map(str::to_owned),
            parent: parent.map(SessionLocation::session_ref),
            orphan: parent.is_none(),
            project_owner: owner.cloned(),
        })
    }

    /// 会话及其全部后代（父在前）。任一成员的父关系冲突或成环时拒绝，避免删除范围不完整。
    pub fn subtree<'a>(&'a self, id: &'a str) -> Result<Vec<&'a str>, String> {
        if !self.nodes.contains_key(id) {
            return Err("未找到该会话".into());
        }
        let mut children = BTreeMap::<&str, Vec<&str>>::new();
        for node in self.nodes.values() {
            for parent in &node.parents {
                children.entry(parent).or_default().push(&node.id);
            }
        }
        let mut ordered = Vec::new();
        let mut pending = vec![id];
        while let Some(id) = pending.pop() {
            if self.cycles.contains(id) || self.nodes.get(id).is_some_and(|node| node.parents.len() > 1) {
                return Err(format!("会话 {id} 的父子关系存在冲突，无法确认删除范围"));
            }
            ordered.push(id);
            pending.extend(children.get(id).into_iter().flatten().rev());
        }
        Ok(ordered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, project: &str, parents: &[&str]) -> Node {
        Node {
            id: id.into(),
            location: Some(SessionLocation { session_id: id.into(), project_dir: project.into(), cwd: Some(format!("/{project}")) }),
            parents: parents.iter().map(|id| (*id).to_owned()).collect(),
            child: !parents.is_empty(),
        }
    }

    #[test]
    fn resolves_parents_owners_and_orphans() {
        let mut unlocated = node("ghost", "p", &[]);
        unlocated.location = None;
        let lineage = Lineage::new([
            node("root", "a", &[]),
            node("child", "b", &["root"]),
            node("leaf", "c", &["child"]),
            node("lost", "c", &["missing"]),
            node("haunted", "c", &["ghost"]),
            unlocated,
            node("x", "c", &["y"]),
            node("y", "c", &["x"]),
            Node { child: true, ..node("unknown", "c", &[]) },
        ]);
        assert!(lineage.subagent("root").is_none());
        let leaf = lineage.subagent("leaf").unwrap();
        assert!(!leaf.orphan);
        assert_eq!(leaf.parent.unwrap().project_dir, "b");
        assert_eq!(leaf.project_owner.unwrap().session_id, "root");
        for id in ["lost", "haunted", "x", "unknown"] {
            let relation = lineage.subagent(id).unwrap();
            assert!(relation.orphan && relation.parent.is_none() && relation.project_owner.is_none(), "{id}");
        }
        assert_eq!(lineage.subagent("lost").unwrap().parent_id.as_deref(), Some("missing"));
    }

    #[test]
    fn subtree_rejects_conflicting_or_cyclic_members() {
        let lineage = Lineage::new([
            node("root", "a", &[]),
            node("child", "a", &["root"]),
            node("leaf", "a", &["child"]),
            node("split", "a", &["root"]),
            node("split", "a", &["other"]),
            node("x", "a", &["y"]),
            node("y", "a", &["x"]),
        ]);
        assert_eq!(lineage.subtree("child").unwrap(), ["child", "leaf"]);
        assert!(lineage.subtree("root").unwrap_err().contains("split"));
        assert!(lineage.subtree("x").is_err());
        assert!(lineage.subtree("missing").is_err());
    }
}
