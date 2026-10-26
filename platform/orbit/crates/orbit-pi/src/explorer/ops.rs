//! Filesystem mutations for the Explorer: create, rename, trash.
//!
//! Pure and synchronous — the caller runs them on the background executor, so
//! no I/O ever touches a frame. Names are validated here, before anything
//! reaches the filesystem, and failures carry a locale key rather than a
//! baked English string.
//!
//! `create_*` never clobbers (they use `create_new` / `create_dir`), and
//! `rename` refuses an existing destination, so a mistyped name can never
//! silently overwrite a file.

use std::fs::{self, OpenOptions};
use std::io;

use std::path::{Path, PathBuf};

/// A user-facing failure. `key` indexes `locales/en.yml`; `detail` carries the
/// OS message for the generic `explorer.err_io` case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpError {
    pub key: &'static str,
    pub detail: Option<String>,
}

impl OpError {
    fn key(key: &'static str) -> Self {
        Self { key, detail: None }
    }

    fn io(error: io::Error) -> Self {
        Self {
            key: "explorer.err_io",
            detail: Some(error.to_string()),
        }
    }

    /// A locale-resolved, user-facing message (OS detail appended for the
    /// generic I/O case).
    pub fn message(&self) -> String {
        let message = crate::i18n::translate(self.key);
        match &self.detail {
            Some(detail) => format!("{message}: {detail}"),
            None => message,
        }
    }
}

/// Trim and validate a single path component typed into the inline prompt.
/// Rejects empties, traversal (`.` / `..`), separators, control characters,
/// and names longer than a filesystem will take. Returns the trimmed name.
pub fn validate_name(raw: &str) -> Result<String, OpError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(OpError::key("explorer.err_name_empty"));
    }
    if name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.chars().any(char::is_control)
    {
        return Err(OpError::key("explorer.err_name_invalid"));
    }
    if name.len() > 255 {
        return Err(OpError::key("explorer.err_name_too_long"));
    }
    Ok(name.to_owned())
}

/// Create an empty file named `name` in `parent`. Fails if it already exists
/// (including as a directory) rather than truncating it.
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, OpError> {
    let name = validate_name(name)?;
    let path = parent.join(&name);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(OpError::key("explorer.err_exists"))
        }
        Err(error) => Err(OpError::io(error)),
    }
}

/// Create an empty directory named `name` in `parent`, one level only.
pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf, OpError> {
    let name = validate_name(name)?;
    let path = parent.join(&name);
    match fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(OpError::key("explorer.err_exists"))
        }
        Err(error) => Err(OpError::io(error)),
    }
}

/// Rename `target` to `name` within its own directory. A no-op when the name
/// is unchanged; refuses to overwrite an existing sibling.
pub fn rename(target: &Path, name: &str) -> Result<PathBuf, OpError> {
    let name = validate_name(name)?;
    let parent = target
        .parent()
        .ok_or_else(|| OpError::key("explorer.err_name_invalid"))?;
    let destination = parent.join(&name);
    if destination == target {
        return Ok(destination);
    }
    if destination.exists() {
        return Err(OpError::key("explorer.err_exists"));
    }
    fs::rename(target, &destination).map_err(OpError::io)?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-explorer-ops-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn validate_rejects_unsafe_names() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("a\nb").is_err());
        assert_eq!(validate_name("  main.rs  ").unwrap(), "main.rs");
        assert_eq!(validate_name("main.rs").unwrap(), "main.rs");
    }

    #[test]
    fn create_file_is_create_new_not_truncate() {
        let root = workspace("create-file");
        let path = create_file(&root, "main.rs").unwrap();
        assert!(path.exists());
        fs::write(&path, "fn main() {}\n").unwrap();

        // A second create must not clobber the existing file.
        let err = create_file(&root, "main.rs").unwrap_err();
        assert_eq!(err.key, "explorer.err_exists");
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn main() {}\n");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn create_dir_is_one_level_and_refuses_existing() {
        let root = workspace("create-dir");
        assert!(create_dir(&root, "src").unwrap().is_dir());
        assert_eq!(
            create_dir(&root, "src").unwrap_err().key,
            "explorer.err_exists"
        );
        // Missing intermediate directories are an error, not created.
        assert!(create_dir(&root, "a/b").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_within_the_same_directory() {
        let root = workspace("rename");
        let original = root.join("old.rs");
        fs::write(&original, "x\n").unwrap();

        let new = rename(&original, "new.rs").unwrap();
        assert_eq!(new, root.join("new.rs"));
        assert!(!original.exists());
        assert!(new.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_refuses_to_overwrite_and_allows_no_op() {
        let root = workspace("rename-collision");
        let original = root.join("a.rs");
        fs::write(&original, "a\n").unwrap();
        fs::write(root.join("b.rs"), "b\n").unwrap();

        assert_eq!(
            rename(&original, "b.rs").unwrap_err().key,
            "explorer.err_exists"
        );
        // Renaming to the same name is a no-op, not a collision.
        assert_eq!(rename(&original, "a.rs").unwrap(), original);
        assert_eq!(fs::read_to_string(root.join("b.rs")).unwrap(), "b\n");
        let _ = fs::remove_dir_all(&root);
    }
}
