/// A directory entry returned by [`FileSystem::list`].
#[derive(Debug, Clone)]
pub struct FsEntry {
    pub name:   String,
    pub is_dir: bool,
}

/// Thin I/O abstraction injected into [`GraphIndexer`].
///
/// All paths passed to these methods are **absolute**.  The trait uses `&self`
/// so implementors can use interior mutability as needed.
pub trait FileSystem {
    /// List the immediate children of `dir`.  Returns an empty `Vec` if `dir`
    /// does not exist or cannot be read.
    fn list(&self, dir: &str) -> Vec<FsEntry>;

    /// Read the UTF-8 content of `path`.  Returns `None` if the file does not
    /// exist or cannot be read.
    fn read(&self, path: &str) -> Option<String>;

    /// Write `content` to `path`, creating parent directories as needed.
    /// Returns `true` on success.
    fn write(&self, path: &str, content: &str) -> bool;
}

// ─── MockFileSystem (tests only) ──────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::{FileSystem, FsEntry};
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    /// Simple in-memory [`FileSystem`] for unit tests.
    pub struct MockFileSystem {
        /// Pre-populated files: absolute path → content.
        pub files: HashMap<String, String>,
        /// Files written by the indexer, captured for assertions.
        pub written: RefCell<HashMap<String, String>>,
    }

    impl MockFileSystem {
        pub fn new() -> Self {
            Self {
                files:   HashMap::new(),
                written: RefCell::new(HashMap::new()),
            }
        }

        pub fn add(&mut self, path: &str, content: &str) {
            self.files.insert(path.to_string(), content.to_string());
        }

        pub fn get_written(&self, path: &str) -> Option<String> {
            self.written.borrow().get(path).cloned()
        }
    }

    impl FileSystem for MockFileSystem {
        fn list(&self, dir: &str) -> Vec<FsEntry> {
            let prefix = format!("{}/", dir);
            let mut seen = HashSet::new();
            let mut entries = Vec::new();

            for path in self.files.keys() {
                if let Some(rest) = path.strip_prefix(&prefix) {
                    // Only direct children: take the first path component.
                    if let Some(component) = rest.split('/').next() {
                        if seen.insert(component.to_string()) {
                            let is_dir = rest.contains('/');
                            entries.push(FsEntry {
                                name:   component.to_string(),
                                is_dir,
                            });
                        }
                    }
                }
            }
            entries
        }

        fn read(&self, path: &str) -> Option<String> {
            self.files.get(path).cloned()
        }

        fn write(&self, path: &str, content: &str) -> bool {
            self.written.borrow_mut().insert(path.to_string(), content.to_string());
            true
        }
    }
}
