use std::path::PathBuf;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SessionState {
    Starting,
    Running,
    WaitingOnYou,
    Idle,
    Parked,
    Closed,
    Error,
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub state: SessionState,
    /// Whether this session is pinned (pinned sessions are never auto-parked).
    pub pinned: bool,
}

#[derive(Default)]
pub struct SessionManager {
    sessions: HashMap<String, Session>,
    order: Vec<String>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&mut self, cwd: PathBuf) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.create_with_id(id, cwd)
    }

    /// Register a session whose id is the provided string — used when the
    /// caller needs the id to equal the `--session-id` passed to `claude`.
    pub fn create_with_id(&mut self, id: String, cwd: PathBuf) -> String {
        let label = cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".to_string());
        self.sessions.insert(
            id.clone(),
            Session { id: id.clone(), label, cwd, state: SessionState::Starting, pinned: false },
        );
        self.order.push(id.clone());
        id
    }

    pub fn get(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn list(&self) -> Vec<Session> {
        self.order.iter().filter_map(|id| self.sessions.get(id).cloned()).collect()
    }

    pub fn set_state(&mut self, id: &str, state: SessionState) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) => { s.state = state; true }
            None => false,
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.order.retain(|x| x != id);
        self.sessions.remove(id).is_some()
    }

    pub fn rename(&mut self, id: &str, new_label: &str) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) => { s.label = new_label.to_string(); true }
            None => false,
        }
    }

    /// Toggle the `pinned` flag for the given session.
    /// Returns the NEW pinned value, or `false` (and no-ops) if `id` is unknown.
    pub fn toggle_pin(&mut self, id: &str) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) => {
                s.pinned = !s.pinned;
                s.pinned
            }
            None => false,
        }
    }

    /// Explicitly set the `pinned` flag.
    /// Returns `true` on success, `false` if the id is unknown.
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) => { s.pinned = pinned; true }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_registers_session_with_label_from_dir() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/Users/m/Desktop/hp-app"));
        let s = m.get(&id).expect("session should exist");
        assert_eq!(s.label, "hp-app");
        assert_eq!(s.state, SessionState::Starting);
    }

    #[test]
    fn list_preserves_creation_order() {
        let mut m = SessionManager::new();
        let a = m.create(PathBuf::from("/tmp/a"));
        let b = m.create(PathBuf::from("/tmp/b"));
        let ids: Vec<String> = m.list().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![a, b]);
    }

    #[test]
    fn set_state_updates_known_session_and_rejects_unknown() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(m.set_state(&id, SessionState::Running));
        assert_eq!(m.get(&id).unwrap().state, SessionState::Running);
        assert!(!m.set_state("nope", SessionState::Running));
    }

    #[test]
    fn remove_deletes_session() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(m.remove(&id));
        assert!(m.get(&id).is_none());
        assert!(!m.remove(&id));
    }

    #[test]
    fn rename_updates_known_session() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/project"));
        assert!(m.rename(&id, "my-project"));
        assert_eq!(m.get(&id).unwrap().label, "my-project");
    }

    #[test]
    fn rename_rejects_unknown_id() {
        let mut m = SessionManager::new();
        assert!(!m.rename("nonexistent-id", "whatever"));
    }

    // ── pin tests ─────────────────────────────────────────────────────────────

    #[test]
    fn new_session_unpinned_by_default() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(!m.get(&id).unwrap().pinned);
    }

    #[test]
    fn toggle_pin_known_id_toggles_true_false_true() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        // starts false → toggle → true
        assert!(m.toggle_pin(&id));
        assert!(m.get(&id).unwrap().pinned);
        // true → toggle → false
        assert!(!m.toggle_pin(&id));
        assert!(!m.get(&id).unwrap().pinned);
        // false → toggle → true
        assert!(m.toggle_pin(&id));
        assert!(m.get(&id).unwrap().pinned);
    }

    #[test]
    fn toggle_pin_unknown_id_returns_false_and_noops() {
        let mut m = SessionManager::new();
        assert!(!m.toggle_pin("nonexistent-id"));
        // Manager still empty — nothing to verify, just no panic.
    }

    #[test]
    fn set_pinned_known_id() {
        let mut m = SessionManager::new();
        let id = m.create(PathBuf::from("/tmp/a"));
        assert!(m.set_pinned(&id, true));
        assert!(m.get(&id).unwrap().pinned);
        assert!(m.set_pinned(&id, false));
        assert!(!m.get(&id).unwrap().pinned);
    }

    #[test]
    fn set_pinned_unknown_id_returns_false() {
        let mut m = SessionManager::new();
        assert!(!m.set_pinned("nope", true));
    }
}
