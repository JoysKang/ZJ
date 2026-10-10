//! Sub-agent subscriptions belong to the ZJ session that spawned them.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Child {
    parent: String,
    idle: bool,
    releasing: bool,
    closed: bool,
    status_version: u64,
}

struct Closing {
    generation: i64,
    discovering: bool,
    closed: bool,
}

#[derive(Default)]
pub(super) struct Subagents {
    children: HashMap<String, Child>,
    closing: HashMap<String, Closing>,
    open: HashSet<String>,
    resuming: HashMap<String, i64>,
}

impl Subagents {
    pub fn close(&mut self, root: &str, generation: i64) {
        self.open.remove(root);
        self.resuming.remove(root);
        self.closing.insert(
            root.to_string(),
            Closing {
                generation,
                discovering: true,
                closed: false,
            },
        );
        self.invalidate_idle(root);
    }

    fn owned_children(&self, root: &str) -> Vec<String> {
        self.children
            .keys()
            .filter(|id| {
                self.closing_owner(id, false)
                    .is_some_and(|(owner, _)| owner == root)
            })
            .cloned()
            .collect()
    }

    fn invalidate_idle(&mut self, root: &str) {
        for id in self.owned_children(root) {
            // Cached idle status or reads from an earlier close must not release a child.
            let child = self.children.get_mut(&id).unwrap();
            child.idle = false;
            child.status_version = child.status_version.wrapping_add(1);
        }
    }

    pub fn close_failed(&mut self, root: &str, generation: i64) {
        if self.current(root, generation) {
            self.closing.remove(root);
            self.open.insert(root.to_string());
        }
    }

    pub fn resume(&mut self, root: &str) {
        self.closing.remove(root);
        self.resuming.remove(root);
        self.open.insert(root.to_string());
    }

    pub fn begin_resume(&mut self, root: &str, request: i64) -> bool {
        if self.resuming.contains_key(root) {
            return false;
        }
        self.resuming.insert(root.to_string(), request);
        true
    }

    pub fn resuming(&self, root: &str, request: i64) -> bool {
        self.resuming.get(root) == Some(&request)
    }

    pub fn resume_failed(&mut self, id: &str, request: i64, generation: i64) -> Option<String> {
        if !self.resuming(id, request) {
            return None;
        }
        self.resuming.remove(id);
        let root = if self.closing.contains_key(id) {
            id.to_string()
        } else {
            self.closing_owner(id, false)?.0
        };
        let closing = self.closing.get_mut(&root)?;
        closing.generation = generation;
        closing.discovering = true;
        self.invalidate_idle(&root);
        Some(root)
    }

    pub fn current(&self, root: &str, generation: i64) -> bool {
        self.closing
            .get(root)
            .is_some_and(|closing| closing.generation == generation)
    }

    pub fn read_version(&mut self, id: &str) -> u64 {
        let child = self.children.entry(id.to_string()).or_default();
        child.status_version = child.status_version.wrapping_add(1);
        child.idle = false;
        child.status_version
    }

    pub fn metadata(&mut self, thread: &Value, version: u64) {
        let Some(id) = thread["id"].as_str() else {
            return;
        };
        self.parent(thread);
        if let Some(child) = self.children.get_mut(id)
            && child.status_version == version
        {
            child.idle = thread["status"]["type"].as_str() == Some("idle");
        }
    }

    fn parent(&mut self, thread: &Value) {
        let Some(id) = thread["id"].as_str() else {
            return;
        };
        let parent = thread["parentThreadId"]
            .as_str()
            .or_else(|| thread["source"]["subAgent"]["thread_spawn"]["parent_thread_id"].as_str());
        if let Some(parent) = parent {
            self.spawned(id, parent);
        }
    }

    pub fn observe(&mut self, thread: &Value) {
        let Some(id) = thread["id"].as_str() else {
            return;
        };
        self.parent(thread);
        if let Some(child) = self.children.get_mut(id)
            && child.closed
        {
            child.closed = false;
            child.releasing = false;
        }
        self.status(id, &thread["status"]);
    }

    pub fn spawned(&mut self, id: &str, parent: &str) {
        if id != parent && !parent.is_empty() {
            let child = self.children.entry(id.to_string()).or_default();
            if child.parent.is_empty() {
                child.parent = parent.to_string();
            }
        }
    }

    pub fn status(&mut self, id: &str, status: &Value) {
        if let Some(child) = self.children.get_mut(id) {
            child.status_version = child.status_version.wrapping_add(1);
            // Unknown, active and approval/user-input waits must keep their subscriptions.
            child.idle = status["type"].as_str() == Some("idle");
        }
    }

    pub fn owner_closing(&self, id: &str) -> Option<(String, i64)> {
        self.closing_owner(id, true)
    }

    // Preserve ownership while a resume temporarily protects the subtree from reclamation.
    fn closing_owner(&self, id: &str, protect_resuming: bool) -> Option<(String, i64)> {
        if self.open.contains(id)
            || (protect_resuming && self.resuming.contains_key(id))
            || self.closing.contains_key(id)
        {
            return None;
        }
        let mut parent = &self.children.get(id)?.parent;
        // Bound malformed/cyclic ancestry without ever treating it as owned.
        for _ in 0..=self.children.len() {
            if self.open.contains(parent)
                || (protect_resuming && self.resuming.contains_key(parent))
            {
                return None;
            }
            if let Some(closing) = self.closing.get(parent) {
                return Some((parent.clone(), closing.generation));
            }
            parent = &self.children.get(parent)?.parent;
        }
        None
    }

    pub fn ready(&self) -> Vec<String> {
        self.children
            .iter()
            .filter(|(id, child)| {
                child.idle && !child.releasing && !child.closed && self.owner_closing(id).is_some()
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn releasing(&mut self, id: &str, success: bool) {
        if let Some(child) = self.children.get_mut(id) {
            child.releasing = success;
        }
    }

    pub fn discovered(&mut self, root: &str, generation: i64) {
        if let Some(closing) = self.closing.get_mut(root)
            && closing.generation == generation
        {
            closing.discovering = false;
            self.prune(root);
        }
    }

    pub fn closed(&mut self, id: &str) {
        if let Some(closing) = self.closing.get_mut(id) {
            closing.closed = true;
        }
        let root = self
            .owner_closing(id)
            .map(|(root, _)| root)
            .or_else(|| self.closing.contains_key(id).then(|| id.to_string()));
        if let Some(child) = self.children.get_mut(id) {
            // Keep ancestry until active grandchildren finish.
            child.closed = true;
            child.idle = false;
            child.status_version = child.status_version.wrapping_add(1);
        }
        if let Some(root) = root {
            self.prune(&root);
        }
    }

    fn prune(&mut self, root: &str) {
        let Some(closing) = self.closing.get(root) else {
            return;
        };
        if closing.discovering || !closing.closed || self.resuming.contains_key(root) {
            return;
        }
        let owned = self.owned_children(root);
        if owned
            .iter()
            .any(|child| !self.children[child].closed || self.resuming.contains_key(child))
        {
            return;
        }
        for child in owned {
            self.children.remove(&child);
        }
        self.closing.remove(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_idle_descendants_of_closed_sessions_are_released() {
        let mut s = Subagents::default();
        for (id, parent, status) in [
            ("done", "a", "idle"),
            ("running", "a", "active"),
            ("approval", "a", "active"),
            ("other", "b", "idle"),
        ] {
            s.observe(&json!({"id": id,"parentThreadId": parent,"status":{"type":status,"activeFlags":["waitingOnApproval"]}}));
        }
        s.spawned("unknown", "a");
        assert!(s.ready().is_empty());
        s.close("a", 1);
        assert!(s.ready().is_empty());
        s.status("done", &json!({"type":"idle"}));
        assert_eq!(s.ready(), ["done"]);
        s.releasing("done", true);
        assert!(s.ready().is_empty());
        s.status("running", &json!({"type":"idle"}));
        assert_eq!(s.ready(), ["running"]);
        s.resume("a");
        assert!(s.ready().is_empty());
        assert!(!s.current("a", 1));
    }

    #[test]
    fn closed_parent_keeps_ancestry_for_running_grandchildren() {
        let mut s = Subagents::default();
        s.observe(&json!({"id":"child","parentThreadId":"a","status":{"type":"idle"}}));
        s.observe(&json!({"id":"grandchild","parentThreadId":"child","status":{"type":"active"}}));
        s.close("a", 1);
        s.discovered("a", 1);
        s.closed("child");
        s.closed("a");
        assert!(s.current("a", 1));
        s.status("grandchild", &json!({"type":"idle"}));
        assert_eq!(s.ready(), ["grandchild"]);
        s.closed("grandchild");
        assert!(s.children.is_empty());
        assert!(s.closing.is_empty());
    }

    #[test]
    fn fast_root_unload_does_not_discard_in_flight_child_discovery() {
        let mut s = Subagents::default();
        s.close("root", 1);
        s.closed("root");
        assert!(s.current("root", 1));
        let version = s.read_version("child");
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}),
            version,
        );
        s.discovered("root", 1);
        assert_eq!(s.ready(), ["child"]);
        s.closed("child");
        assert!(!s.current("root", 1));
    }

    #[test]
    fn a_late_metadata_read_cannot_replace_a_newer_active_status() {
        let mut s = Subagents::default();
        s.close("root", 1);
        let version = s.read_version("child");
        s.status(
            "child",
            &json!({"type":"active","activeFlags":["waitingOnApproval"]}),
        );
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}),
            version,
        );
        assert!(s.ready().is_empty());
        let version = s.read_version("child");
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}),
            version,
        );
        assert_eq!(s.ready(), ["child"]);
    }

    #[test]
    fn closing_invalidates_metadata_requested_before_the_close() {
        let mut s = Subagents::default();
        s.spawned("child", "root");
        let version = s.read_version("child");
        s.close("root", 1);
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}),
            version,
        );
        assert!(s.ready().is_empty());
    }

    #[test]
    fn older_metadata_cannot_overwrite_the_latest_read() {
        let mut s = Subagents::default();
        s.close("root", 1);
        let old = s.read_version("child");
        let new = s.read_version("child");
        assert_ne!(old, new);
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"active"}}),
            new,
        );
        s.metadata(
            &json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}),
            old,
        );
        assert!(s.ready().is_empty());
    }

    #[test]
    fn failed_child_resume_preserves_closed_ancestors_and_restarts_discovery() {
        let mut s = Subagents::default();
        s.spawned("child", "root");
        s.spawned("grandchild", "child");
        s.close("root", 1);
        s.closed("child");
        s.closed("grandchild");
        s.closed("root");
        assert!(s.begin_resume("child", 2));
        assert!(!s.begin_resume("child", 3));
        s.discovered("root", 1);
        assert!(s.current("root", 1));
        assert_eq!(s.resume_failed("child", 2, 4), Some("root".into()));
        assert!(!s.current("root", 1));
        assert!(s.current("root", 4));
        s.observe(&json!({"id":"child","parentThreadId":"root","status":{"type":"idle"}}));
        assert_eq!(s.ready(), ["child"]);
    }

    #[test]
    fn late_resume_failure_cannot_replace_a_newer_close_or_resume() {
        let mut s = Subagents::default();
        s.close("root", 1);
        assert!(s.begin_resume("root", 2));
        s.close("root", 3);
        assert!(s.begin_resume("root", 4));
        assert_eq!(s.resume_failed("root", 2, 5), None);
        assert!(s.current("root", 3));
        assert!(s.resuming("root", 4));
        s.resume("root");
        assert_eq!(s.resume_failed("root", 4, 6), None);
        assert!(!s.current("root", 3));
    }

    #[test]
    fn another_open_zj_session_owns_its_descendants() {
        let mut s = Subagents::default();
        s.observe(&json!({"id":"child","parentThreadId":"a","status":{"type":"idle"}}));
        s.observe(&json!({"id":"grandchild","parentThreadId":"child","status":{"type":"idle"}}));
        s.resume("child");
        s.close("a", 1);
        assert!(s.ready().is_empty());
        s.closed("unrelated");
        assert!(s.current("a", 1));
        s.close("child", 2);
        assert!(s.ready().is_empty());
        s.status("grandchild", &json!({"type":"idle"}));
        assert_eq!(s.ready(), ["grandchild"]);
    }

    #[test]
    fn unknown_ownership_cycles_and_failed_close_are_safe() {
        let mut s = Subagents::default();
        s.close("a", 1);
        s.observe(&json!({"id":"legacy","source":{"subAgent":{"thread_spawn":{"parent_thread_id":"a"}}},"status":{"type":"idle"}}));
        s.observe(&json!({"id":"orphan","parentThreadId":"missing","status":{"type":"idle"}}));
        s.spawned("cycle1", "cycle2");
        s.spawned("cycle2", "cycle1");
        s.status("cycle1", &json!({"type":"idle"}));
        assert_eq!(s.ready(), ["legacy"]);
        s.releasing("legacy", true);
        assert!(s.ready().is_empty());
        s.releasing("legacy", false);
        assert_eq!(s.ready(), ["legacy"]);
        s.close("a", 2);
        assert!(!s.current("a", 1));
        assert!(s.current("a", 2));
    }
}
