//! Session persistence for `vktr acp`, so an editor can reload a session after the agent restarts.
//!
//! Viktor keeps the thread itself server-side; what an agent restart loses is only the mapping
//! from the editor's session id to that thread, and the transcript the editor wants replayed on
//! `session/load`. Both live in one small JSON file per session under
//! `$VKTR_HOME/acp/sessions/`, written owner-only because it holds the conversation.

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};

/// Oldest transcript entries are dropped past this, so a long session cannot grow without bound.
const MAX_ENTRIES: usize = 400;

/// Longest title derived from the first prompt.
const MAX_TITLE_CHARS: usize = 80;

/// One replayable transcript entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    User {
        text: String,
    },
    Agent {
        text: String,
    },
    /// A local tool call, stored without its content (diffs hold whole files).
    Tool {
        call: Box<acp::ToolCall>,
    },
}

/// Everything `vktr acp` needs to bring a session back.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSession {
    pub session_id: String,
    pub cwd: PathBuf,
    /// The Viktor thread: on Viktor a Responses response id *is* the durable thread id.
    pub thread_id: Option<String>,
    pub title: Option<String>,
    /// RFC 3339.
    pub updated_at: Option<String>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Outputs owed to Viktor for tool calls it made, not yet accepted by Viktor. A thread with an
    /// unanswered call cannot be continued, so these go out ahead of the next message; they are
    /// dropped only once Viktor accepts a request carrying them. Stored so that a failed request,
    /// a cancel or an agent restart cannot strand the thread.
    #[serde(default)]
    pub owed_outputs: Vec<OwedOutput>,
}

/// One `function_call_output` still owed to Viktor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwedOutput {
    pub call_id: String,
    pub output: String,
}

impl StoredSession {
    pub fn new(session_id: impl Into<String>, cwd: PathBuf) -> Self {
        Self {
            session_id: session_id.into(),
            cwd,
            ..Self::default()
        }
    }

    pub fn push(&mut self, entry: Entry) {
        if self.title.is_none()
            && let Entry::User { text } = &entry
        {
            let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            if !line.is_empty() {
                self.title = Some(line.trim().chars().take(MAX_TITLE_CHARS).collect());
            }
        }
        self.entries.push(entry);
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        if excess > 0 {
            self.entries.drain(..excess);
        }
    }
}

/// A directory of stored sessions.
#[derive(Clone, Debug)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Where sessions live for a given vktr home.
    pub fn in_home(home: &Path) -> Self {
        Self::new(home.join("acp").join("sessions"))
    }

    /// Session ids come from the editor on `session/load`; only ids that are safe as a file
    /// name are accepted, so a crafted id cannot address a path outside the store.
    fn path_for(&self, session_id: &str) -> Option<PathBuf> {
        let safe = !session_id.is_empty()
            && session_id.len() <= 128
            && session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        safe.then(|| self.dir.join(format!("{session_id}.json")))
    }

    pub fn load(&self, session_id: &str) -> Option<StoredSession> {
        let path = self.path_for(session_id)?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write atomically (temp file + rename) with owner-only permissions.
    pub fn save(&self, session: &mut StoredSession) -> std::io::Result<()> {
        let path = self
            .path_for(&session.session_id)
            .ok_or_else(|| std::io::Error::other("session id is not a safe file name"))?;
        session.updated_at = Some(chrono::Utc::now().to_rfc3339());
        create_private_dir(&self.dir)?;
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, serde_json::to_vec(session)?.as_slice())?;
        std::fs::rename(&tmp, &path)
    }

    /// Stored sessions, most recently updated first, optionally only those for one workspace.
    pub fn list(&self, cwd: Option<&Path>) -> Vec<StoredSession> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut sessions: Vec<StoredSession> = dir
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|text| serde_json::from_str::<StoredSession>(&text).ok())
            .filter(|s| cwd.is_none_or(|cwd| s.cwd == cwd))
            .collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_session_loads_back_with_its_thread_and_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let mut session = StoredSession::new("abc-123", PathBuf::from("/work"));
        session.thread_id = Some("thread-1".to_owned());
        session.push(Entry::User {
            text: "\nfix the build\nplease".to_owned(),
        });
        session.push(Entry::Agent {
            text: "done".to_owned(),
        });
        store.save(&mut session).expect("save");

        let loaded = store.load("abc-123").expect("load");
        assert_eq!(loaded.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(loaded.title.as_deref(), Some("fix the build"));
        assert_eq!(loaded.entries, session.entries);
        assert!(loaded.updated_at.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join("abc-123.json"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the transcript must be owner-only");
        }
    }

    #[test]
    fn an_id_that_is_not_a_plain_file_name_is_never_read_or_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path().join("sessions"));
        for id in ["../escape", "a/b", "", "x.json", "..", "id with space"] {
            assert!(store.load(id).is_none(), "{id}");
            assert!(
                store
                    .save(&mut StoredSession::new(id, PathBuf::from("/w")))
                    .is_err(),
                "{id}"
            );
        }
        assert!(!dir.path().join("escape.json").exists());
    }

    #[test]
    fn the_list_is_filtered_by_workspace_and_newest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        for (id, cwd) in [("s1", "/a"), ("s2", "/b"), ("s3", "/a")] {
            store
                .save(&mut StoredSession::new(id, PathBuf::from(cwd)))
                .expect("save");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let ids: Vec<String> = store
            .list(Some(Path::new("/a")))
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        assert_eq!(ids, ["s3", "s1"]);
        assert_eq!(store.list(None).len(), 3);
    }

    #[test]
    fn a_long_transcript_keeps_only_its_newest_entries() {
        let mut session = StoredSession::new("s", PathBuf::from("/w"));
        for i in 0..(MAX_ENTRIES + 10) {
            session.push(Entry::Agent {
                text: i.to_string(),
            });
        }
        assert_eq!(session.entries.len(), MAX_ENTRIES);
        assert_eq!(
            session.entries.first(),
            Some(&Entry::Agent {
                text: "10".to_owned()
            })
        );
    }
}
