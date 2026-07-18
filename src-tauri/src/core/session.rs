use std::path::PathBuf;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    Starting,
    Running,
    WaitingOnYou,
    Idle,
    Parked,
    Error,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub state: SessionState,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    order: Vec<String>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), order: Vec::new() }
    }

    pub fn create(&mut self, cwd: PathBuf) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let label = cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".to_string());
        self.sessions.insert(
            id.clone(),
            Session { id: id.clone(), label, cwd, state: SessionState::Starting },
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
}
