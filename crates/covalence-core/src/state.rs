//! The state tree (state spec §1).
//!
//! Values live at leaves. A path is stored in ID form, `{conn, path}`, with the
//! path a segment array (ADR 0017). Setting a map value flattens it into one
//! leaf per entry, so every leaf carries its own metadata; arrays, scalars and
//! empty maps are leaves. Every mutation returns the set of leaf paths it
//! changed: the dirty set the binding evaluator will consume (§1.5).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use covalence_protocol::Value;
use covalence_protocol::state::{PatchOp, Path, PathSegment, PathValue};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast};

use crate::journal::Seq;

/// A connection ID (ULID).
pub type ConnId = String;

/// A state path in stored form.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StatePath {
    pub conn: ConnId,
    pub path: Path,
}

impl StatePath {
    pub fn new(conn: impl Into<ConnId>, path: Path) -> Self {
        StatePath {
            conn: conn.into(),
            path,
        }
    }

    /// The root of a connection's subtree.
    pub fn root(conn: impl Into<ConnId>) -> Self {
        Self::new(conn, Vec::new())
    }

    pub fn is_within(&self, ancestor: &StatePath) -> bool {
        self.conn == ancestor.conn && self.path.starts_with(&ancestor.path)
    }

    fn child(&self, segment: PathSegment) -> Self {
        let mut path = self.path.clone();
        path.push(segment);
        StatePath::new(self.conn.clone(), path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Good,
    /// The source disconnected; the last value is kept.
    Stale,
    /// Never received.
    Unknown,
}

/// Who set a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Conn(ConnId),
    Core,
    User,
}

/// Per-leaf metadata (§1.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// Wall clock of the last change, Unix epoch milliseconds.
    pub ts: u64,
    /// Journal seq of the entry that set the value.
    pub seq: Seq,
    pub quality: Quality,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Leaf {
    pub value: Value,
    pub meta: Meta,
}

/// Leaf paths changed by one mutation.
pub type Dirty = BTreeSet<StatePath>;

/// What [`StateTree::probe`] found at a path.
#[derive(Debug)]
pub enum Probe<'a> {
    Leaf(&'a Leaf),
    /// Leaves below it, none at it.
    Branch,
    Missing,
}

#[derive(Debug, Default)]
pub struct StateTree {
    // Ordered so that a path's descendants are contiguous right after it.
    leaves: BTreeMap<StatePath, Leaf>,
}

impl StateTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn leaf(&self, path: &StatePath) -> Option<&Leaf> {
        self.leaves.get(path)
    }

    /// Every leaf at or below `path`, in path order.
    pub fn leaves_under<'a>(
        &'a self,
        path: &'a StatePath,
    ) -> impl Iterator<Item = (&'a StatePath, &'a Leaf)> + 'a {
        self.leaves
            .range(path..)
            .take_while(move |(k, _)| k.is_within(path))
    }

    /// What is at `path`, without rebuilding anything.
    pub fn probe(&self, path: &StatePath) -> Probe<'_> {
        if let Some(leaf) = self.leaves.get(path) {
            return Probe::Leaf(leaf);
        }
        match self.leaves_under(path).next() {
            Some(_) => Probe::Branch,
            None => Probe::Missing,
        }
    }

    /// The segments directly below `path`, in path order.
    pub fn children(&self, path: &StatePath) -> Vec<PathSegment> {
        let mut out: Vec<PathSegment> = Vec::new();
        for (k, _) in self.leaves_under(path) {
            if let Some(seg) = k.path.get(path.path.len())
                && out.last() != Some(seg)
            {
                out.push(seg.clone());
            }
        }
        out
    }

    /// The value at `path`: a leaf, or a map rebuilt from the leaves below it.
    /// `None` if nothing is there.
    pub fn value(&self, path: &StatePath) -> Option<Value> {
        if let Some(leaf) = self.leaves.get(path) {
            return Some(leaf.value.clone());
        }
        let mut node = Node::Branch(BTreeMap::new());
        let mut found = false;
        for (k, leaf) in self.leaves_under(path) {
            node.insert(&k.path[path.path.len()..], leaf.value.clone());
            found = true;
        }
        found.then(|| node.into_value())
    }

    /// Sets `value` at `path`, replacing whatever was at, above or below it.
    pub fn set(&mut self, path: &StatePath, value: &Value, meta: &Meta) -> Dirty {
        let mut dirty = Dirty::new();
        self.set_into(path, value, meta, &mut dirty);
        dirty
    }

    /// Removes the value at `path` and everything below it.
    pub fn delete(&mut self, path: &StatePath) -> Dirty {
        let mut dirty = Dirty::new();
        self.delete_into(path, &mut dirty);
        dirty
    }

    /// Applies a `state.patch`. Paths are relative to the connection root.
    pub fn apply_patch(&mut self, conn: &str, ops: &[PatchOp], meta: &Meta) -> Dirty {
        let mut dirty = Dirty::new();
        for op in ops {
            match op {
                PatchOp::Set(PathValue { p, v }) => {
                    self.set_into(&StatePath::new(conn, p.clone()), v, meta, &mut dirty)
                }
                PatchOp::Delete(d) => {
                    self.delete_into(&StatePath::new(conn, d.p.clone()), &mut dirty)
                }
            }
        }
        dirty
    }

    /// Applies a `state.snapshot`: the connection's subtree becomes exactly `values`.
    pub fn replace_subtree(&mut self, conn: &str, values: &[PathValue], meta: &Meta) -> Dirty {
        let root = StatePath::root(conn);
        let mut fresh = StateTree::new();
        for PathValue { p, v } in values {
            fresh.set(&StatePath::new(conn, p.clone()), v, meta);
        }
        let old: Vec<StatePath> = self.leaves_under(&root).map(|(k, _)| k.clone()).collect();
        let mut dirty = Dirty::new();
        for k in old {
            if !fresh.leaves.contains_key(&k) {
                self.leaves.remove(&k);
                dirty.insert(k);
            }
        }
        for (k, leaf) in fresh.leaves {
            self.put(k, leaf, &mut dirty);
        }
        dirty
    }

    /// Sets the quality of every leaf under `subtree`, keeping the values.
    pub fn set_quality(&mut self, subtree: &StatePath, quality: Quality) -> Dirty {
        let mut dirty = Dirty::new();
        let keys: Vec<StatePath> = self
            .leaves_under(subtree)
            .filter(|(_, leaf)| leaf.meta.quality != quality)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            if let Some(leaf) = self.leaves.get_mut(&k) {
                leaf.meta.quality = quality;
            }
            dirty.insert(k);
        }
        dirty
    }

    fn set_into(&mut self, path: &StatePath, value: &Value, meta: &Meta, dirty: &mut Dirty) {
        // A leaf above the path is replaced by the new branch.
        for len in 0..path.path.len() {
            let ancestor = StatePath::new(path.conn.clone(), path.path[..len].to_vec());
            if self.leaves.remove(&ancestor).is_some() {
                dirty.insert(ancestor);
            }
        }
        let mut fresh = BTreeMap::new();
        flatten(path, value, &mut fresh);
        let stale: Vec<StatePath> = self
            .leaves_under(path)
            .filter(|(k, _)| !fresh.contains_key(*k))
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            self.leaves.remove(&k);
            dirty.insert(k);
        }
        for (k, value) in fresh {
            let leaf = Leaf {
                value,
                meta: meta.clone(),
            };
            self.put(k, leaf, dirty);
        }
    }

    fn delete_into(&mut self, path: &StatePath, dirty: &mut Dirty) {
        let keys: Vec<StatePath> = self.leaves_under(path).map(|(k, _)| k.clone()).collect();
        for k in keys {
            self.leaves.remove(&k);
            dirty.insert(k);
        }
    }

    /// Inserts a leaf. An identical value from the same source with the same
    /// quality is not a change, and keeps its original metadata.
    fn put(&mut self, path: StatePath, leaf: Leaf, dirty: &mut Dirty) {
        if let Some(old) = self.leaves.get(&path)
            && old.value == leaf.value
            && old.meta.quality == leaf.meta.quality
            && old.meta.source == leaf.meta.source
        {
            return;
        }
        self.leaves.insert(path.clone(), leaf);
        dirty.insert(path);
    }
}

/// The live state tree, shared between tasks, with a feed of dirty sets.
pub struct LiveState {
    tree: Mutex<StateTree>,
    dirty: broadcast::Sender<Arc<Dirty>>,
    turns: Arc<Turns>,
}

/// How far each [`TurnFeed`] has got, so that a writer can wait for a change
/// to be seen on its own (§1.5): control input's edges (surfaces spec §4.1).
#[derive(Default)]
struct Turns {
    /// Dirty sets published so far. Only changed with the tree locked.
    published: AtomicU64,
    feeds: Mutex<Vec<Weak<AtomicU64>>>,
    done: Notify,
}

impl Default for LiveState {
    fn default() -> Self {
        let (dirty, _) = broadcast::channel(1024);
        LiveState {
            tree: Mutex::new(StateTree::new()),
            dirty,
            turns: Arc::default(),
        }
    }
}

impl LiveState {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, StateTree> {
        self.tree.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the tree. Don't block inside `f`.
    pub fn read<R>(&self, f: impl FnOnce(&StateTree) -> R) -> R {
        f(&self.lock())
    }

    pub fn value(&self, path: &StatePath) -> Option<Value> {
        self.read(|tree| tree.value(path))
    }

    pub fn leaf(&self, path: &StatePath) -> Option<Leaf> {
        self.read(|tree| tree.leaf(path).cloned())
    }

    /// Mutates the tree; `f` returns its result and the paths it changed,
    /// which are published to subscribers if there are any.
    pub fn mutate<R>(&self, f: impl FnOnce(&mut StateTree) -> (R, Dirty)) -> R {
        self.mutate_turn(f).0
    }

    /// [`mutate`](Self::mutate), and the turn it published, for
    /// [`seen`](Self::seen).
    pub fn mutate_turn<R>(&self, f: impl FnOnce(&mut StateTree) -> (R, Dirty)) -> (R, u64) {
        let mut tree = self.lock();
        let (result, dirty) = f(&mut tree);
        if !dirty.is_empty() {
            self.turns.published.fetch_add(1, Ordering::SeqCst);
            let _ = self.dirty.send(Arc::new(dirty));
        }
        (result, self.turns.published.load(Ordering::SeqCst))
    }

    /// Dirty sets, one per mutation, in order (§1.5).
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Dirty>> {
        self.dirty.subscribe()
    }

    /// Dirty sets for a consumer that says when it's done with them, so a
    /// writer can wait with [`seen`](Self::seen).
    pub fn subscribe_turns(&self) -> TurnFeed {
        let _tree = self.lock();
        let received = self.turns.published.load(Ordering::SeqCst);
        let done = Arc::new(AtomicU64::new(received));
        let mut feeds = self.turns.feeds.lock().unwrap_or_else(|e| e.into_inner());
        feeds.retain(|f| f.strong_count() > 0);
        feeds.push(Arc::downgrade(&done));
        TurnFeed {
            feed: self.dirty.subscribe(),
            received,
            done,
            turns: self.turns.clone(),
        }
    }

    /// Waits until every [`TurnFeed`] is done with `turn` and what came
    /// before it, or for `limit`. Whether they all were.
    pub async fn seen(&self, turn: u64, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let done = self.turns.done.notified();
            tokio::pin!(done);
            done.as_mut().enable();
            let behind = {
                let feeds = self.turns.feeds.lock().unwrap_or_else(|e| e.into_inner());
                feeds
                    .iter()
                    .filter_map(Weak::upgrade)
                    .any(|f| f.load(Ordering::SeqCst) < turn)
            };
            if !behind {
                return true;
            }
            tokio::select! {
                () = done => {}
                () = tokio::time::sleep_until(deadline) => return false,
            }
        }
    }
}

/// A [`LiveState`] feed whose consumer calls [`done`](Self::done) after
/// acting on what it received.
pub struct TurnFeed {
    feed: broadcast::Receiver<Arc<Dirty>>,
    /// Turns received so far, skipped ones included.
    received: u64,
    done: Arc<AtomicU64>,
    turns: Arc<Turns>,
}

impl TurnFeed {
    pub async fn recv(&mut self) -> Result<Arc<Dirty>, broadcast::error::RecvError> {
        let next = self.feed.recv().await;
        self.count(
            next.as_ref().err().and_then(|e| match e {
                broadcast::error::RecvError::Lagged(n) => Some(*n),
                broadcast::error::RecvError::Closed => None,
            }),
            next.is_ok(),
        );
        next
    }

    pub fn try_recv(&mut self) -> Result<Arc<Dirty>, broadcast::error::TryRecvError> {
        let next = self.feed.try_recv();
        self.count(
            next.as_ref().err().and_then(|e| match e {
                broadcast::error::TryRecvError::Lagged(n) => Some(*n),
                _ => None,
            }),
            next.is_ok(),
        );
        next
    }

    fn count(&mut self, lagged: Option<u64>, received: bool) {
        self.received += lagged.unwrap_or(0) + u64::from(received);
    }

    /// Done with everything received so far.
    pub fn done(&self) {
        self.done.store(self.received, Ordering::SeqCst);
        self.turns.done.notify_waiters();
    }
}

/// A map key as a path segment, if it can be one.
fn segment(key: &Value) -> Option<PathSegment> {
    match key {
        Value::String(s) => s.as_str().map(|s| PathSegment::Key(s.to_owned())),
        Value::Integer(i) => i.as_i64().map(PathSegment::Index),
        _ => None,
    }
}

fn flatten(path: &StatePath, value: &Value, out: &mut BTreeMap<StatePath, Value>) {
    if let Value::Map(entries) = value
        && !entries.is_empty()
        && entries.iter().all(|(k, _)| segment(k).is_some())
    {
        for (k, v) in entries {
            let seg = segment(k).expect("checked above");
            flatten(&path.child(seg), v, out);
        }
        return;
    }
    out.insert(path.clone(), value.clone());
}

enum Node {
    Leaf(Value),
    Branch(BTreeMap<PathSegment, Node>),
}

impl Node {
    fn insert(&mut self, rel: &[PathSegment], value: Value) {
        let Node::Branch(children) = self else {
            return;
        };
        match rel {
            [] => *self = Node::Leaf(value),
            [first, rest @ ..] => children
                .entry(first.clone())
                .or_insert_with(|| Node::Branch(BTreeMap::new()))
                .insert(rest, value),
        }
    }

    fn into_value(self) -> Value {
        match self {
            Node::Leaf(v) => v,
            Node::Branch(children) => Value::Map(
                children
                    .into_iter()
                    .map(|(k, node)| {
                        let key = match k {
                            PathSegment::Index(i) => Value::from(i),
                            PathSegment::Key(s) => Value::from(s),
                        };
                        (key, node.into_value())
                    })
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use covalence_protocol::True;
    use covalence_protocol::state::PathDelete;

    use super::*;

    fn p(segs: &[&str]) -> Path {
        segs.iter()
            .map(|s| match s.parse() {
                Ok(i) => PathSegment::Index(i),
                Err(_) => PathSegment::Key((*s).to_owned()),
            })
            .collect()
    }

    fn sp(segs: &[&str]) -> StatePath {
        StatePath::new("c1", p(segs))
    }

    fn meta(seq: Seq) -> Meta {
        Meta {
            ts: 1,
            seq,
            quality: Quality::Good,
            source: Source::Conn("c1".into()),
        }
    }

    fn map(entries: Vec<(Value, Value)>) -> Value {
        Value::Map(entries)
    }

    fn outputs() -> Value {
        map(vec![(
            "outputs".into(),
            map(vec![
                (Value::from(0), map(vec![("source".into(), Value::from(0))])),
                (Value::from(1), map(vec![("source".into(), Value::from(2))])),
            ]),
        )])
    }

    #[test]
    fn flattens_maps_into_leaves_and_rebuilds_them() {
        let mut tree = StateTree::new();
        let dirty = tree.set(&sp(&[]), &outputs(), &meta(1));
        assert_eq!(
            dirty,
            Dirty::from([
                sp(&["outputs", "0", "source"]),
                sp(&["outputs", "1", "source"])
            ])
        );
        assert_eq!(tree.value(&sp(&[])), Some(outputs()));
        assert_eq!(
            tree.value(&sp(&["outputs", "1", "source"])),
            Some(Value::from(2))
        );
        assert_eq!(tree.value(&sp(&["inputs"])), None);
    }

    #[test]
    fn patch_reports_only_changed_leaves() {
        let mut tree = StateTree::new();
        tree.set(&sp(&[]), &outputs(), &meta(1));
        let ops = vec![
            PatchOp::Set(PathValue {
                p: p(&["outputs", "1", "source"]),
                v: Value::from(3),
            }),
            PatchOp::Set(PathValue {
                p: p(&["outputs", "0", "source"]),
                v: Value::from(0),
            }),
        ];
        let dirty = tree.apply_patch("c1", &ops, &meta(2));
        assert_eq!(dirty, Dirty::from([sp(&["outputs", "1", "source"])]));
        let leaf = tree.leaf(&sp(&["outputs", "1", "source"])).unwrap();
        assert_eq!((leaf.value.clone(), leaf.meta.seq), (Value::from(3), 2));
        // The unchanged leaf keeps the seq that set it.
        assert_eq!(
            tree.leaf(&sp(&["outputs", "0", "source"]))
                .unwrap()
                .meta
                .seq,
            1
        );
    }

    #[test]
    fn set_replaces_ancestors_and_descendants() {
        let mut tree = StateTree::new();
        tree.set(&sp(&["a"]), &Value::from(1), &meta(1));
        let dirty = tree.set(&sp(&["a", "b"]), &Value::from(2), &meta(2));
        assert_eq!(dirty, Dirty::from([sp(&["a"]), sp(&["a", "b"])]));
        let dirty = tree.set(&sp(&["a"]), &Value::from(3), &meta(3));
        assert_eq!(dirty, Dirty::from([sp(&["a"]), sp(&["a", "b"])]));
        assert_eq!(tree.value(&sp(&["a"])), Some(Value::from(3)));
    }

    #[test]
    fn delete_removes_subtree_only() {
        let mut tree = StateTree::new();
        tree.set(&sp(&[]), &outputs(), &meta(1));
        tree.set(&sp(&["outputsX"]), &Value::from(9), &meta(1));
        let ops = vec![PatchOp::Delete(PathDelete {
            p: p(&["outputs"]),
            del: True,
        })];
        let dirty = tree.apply_patch("c1", &ops, &meta(2));
        assert_eq!(dirty.len(), 2);
        assert_eq!(tree.value(&sp(&["outputs"])), None);
        assert_eq!(tree.value(&sp(&["outputsX"])), Some(Value::from(9)));
    }

    #[test]
    fn snapshot_replaces_the_connection_subtree_only() {
        let mut tree = StateTree::new();
        tree.set(&sp(&[]), &outputs(), &meta(1));
        tree.set(&StatePath::new("c2", p(&["x"])), &Value::from(1), &meta(1));
        let values = vec![PathValue {
            p: p(&["outputs", "0", "source"]),
            v: Value::from(0),
        }];
        let dirty = tree.replace_subtree("c1", &values, &meta(5));
        assert_eq!(dirty, Dirty::from([sp(&["outputs", "1", "source"])]));
        assert_eq!(tree.leaves_under(&StatePath::root("c1")).count(), 1);
        assert_eq!(
            tree.value(&StatePath::new("c2", p(&["x"]))),
            Some(Value::from(1))
        );
    }

    #[test]
    fn stale_keeps_values_and_a_snapshot_makes_them_good_again() {
        let mut tree = StateTree::new();
        tree.set(&sp(&[]), &outputs(), &meta(1));
        let dirty = tree.set_quality(&StatePath::root("c1"), Quality::Stale);
        assert_eq!(dirty.len(), 2);
        assert!(
            tree.set_quality(&StatePath::root("c1"), Quality::Stale)
                .is_empty()
        );
        let leaf = tree.leaf(&sp(&["outputs", "1", "source"])).unwrap();
        assert_eq!(
            (leaf.value.clone(), leaf.meta.quality),
            (Value::from(2), Quality::Stale)
        );

        let values = vec![PathValue {
            p: Vec::new(),
            v: outputs(),
        }];
        let dirty = tree.replace_subtree("c1", &values, &meta(9));
        assert_eq!(dirty.len(), 2);
        let leaf = tree.leaf(&sp(&["outputs", "1", "source"])).unwrap();
        assert_eq!((leaf.meta.quality, leaf.meta.seq), (Quality::Good, 9));
    }

    #[test]
    fn keys_that_are_not_identifiers_stay_whole() {
        let mut tree = StateTree::new();
        let path = StatePath::new(
            "c1",
            vec![
                PathSegment::Key("outputs".into()),
                PathSegment::Key("SDI 4".into()),
            ],
        );
        tree.set(&path, &Value::from("a.b"), &meta(1));
        assert_eq!(tree.value(&path), Some(Value::from("a.b")));
    }
}
