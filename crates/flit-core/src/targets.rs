//! Turning what a user dropped on the app into send targets.
//!
//! A folder is not one item: it is every regular file under it, named relative
//! to its parent so the receiver can rebuild the tree. This lives in the engine
//! rather than the CLI because every shell has to answer the same questions the
//! same way: what counts as sendable, what is silently skipped, and what order
//! the items travel in.

use std::path::{Path, PathBuf};

use flit_client::SendTarget;

/// Why a set of paths could not be turned into targets.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    /// A path could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// What the operating system said.
        #[source]
        source: std::io::Error,
    },
    /// A path exists but is neither a regular file nor a folder.
    #[error("{0} is not a file or a directory")]
    NotAFile(PathBuf),
    /// `name` was used with something other than exactly one file.
    #[error("a name only applies to one file, not to a folder or a batch")]
    NameNeedsOneFile,
    /// Nothing sendable was found.
    #[error("nothing to send")]
    Empty,
}

/// What a walk found.
#[derive(Debug, Clone, Default)]
pub struct Collected {
    /// Files to send, in the order the receiver will see them.
    pub targets: Vec<SendTarget>,
    /// Entries that were skipped because they are not regular files:
    /// symlinks, sockets, devices, and folders that could not be entered.
    ///
    /// Reported instead of silent because "my file is missing" is a support
    /// question, and following links can escape the folder or loop forever.
    pub skipped: usize,
}

/// Expand paths into one target per file.
///
/// `name` renames a single file and is refused for folders or several paths,
/// so a caller cannot silently send a folder under one name.
pub fn collect_targets(paths: &[PathBuf], name: Option<&str>) -> Result<Collected, CollectError> {
    if name.is_some() && paths.len() > 1 {
        return Err(CollectError::NameNeedsOneFile);
    }

    let mut collected = Collected::default();
    for path in paths {
        let metadata = std::fs::metadata(path).map_err(|source| CollectError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.is_dir() {
            if name.is_some() {
                return Err(CollectError::NameNeedsOneFile);
            }
            walk_folder(path, &mut collected)?;
        } else if metadata.is_file() {
            collected.targets.push(SendTarget {
                path: path.clone(),
                name: name.map(str::to_owned),
            });
        } else {
            return Err(CollectError::NotAFile(path.clone()));
        }
    }

    collected
        .targets
        .sort_by(|left, right| left.path.cmp(&right.path));
    if collected.targets.is_empty() {
        return Err(CollectError::Empty);
    }
    Ok(collected)
}

/// Add every regular file under `folder`, named relative to its parent.
fn walk_folder(folder: &Path, collected: &mut Collected) -> Result<(), CollectError> {
    let base = folder
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CollectError::NotAFile(folder.to_path_buf()))?
        .to_string();

    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|source| CollectError::Io {
            path: dir.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| CollectError::Io {
                path: dir.clone(),
                source,
            })?;
            let kind = entry.file_type().map_err(|source| CollectError::Io {
                path: entry.path(),
                source,
            })?;
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                let full = entry.path();
                // `relative` must be owned before `full` moves into the target.
                let relative = full
                    .strip_prefix(folder)
                    .unwrap_or(&full)
                    .to_string_lossy()
                    .replace('\\', "/");
                collected.targets.push(SendTarget {
                    path: full,
                    name: Some(format!("{base}/{relative}")),
                });
            } else {
                collected.skipped += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let unique = format!(
            "flit-targets-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_folder_becomes_one_target_per_file_with_its_tree() {
        let root = scratch("walk");
        std::fs::create_dir_all(root.join("shots/day one")).unwrap();
        std::fs::write(root.join("shots/a.jpg"), b"a").unwrap();
        std::fs::write(root.join("shots/day one/b.jpg"), b"b").unwrap();

        let collected = collect_targets(&[root.join("shots")], None).unwrap();
        let names: Vec<String> = collected
            .targets
            .iter()
            .map(|target| target.name.clone().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["shots/a.jpg".to_string(), "shots/day one/b.jpg".to_string()]
        );
        assert_eq!(collected.skipped, 0);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn several_files_keep_their_own_names() {
        let root = scratch("files");
        let one = root.join("one.txt");
        let two = root.join("two.txt");
        std::fs::write(&one, b"1").unwrap();
        std::fs::write(&two, b"2").unwrap();

        let collected = collect_targets(&[one.clone(), two.clone()], None).unwrap();
        assert_eq!(collected.targets.len(), 2);
        assert!(collected.targets.iter().all(|target| target.name.is_none()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_name_for_a_folder_is_refused() {
        let root = scratch("named-folder");
        std::fs::write(root.join("a.txt"), b"a").unwrap();

        let result = collect_targets(std::slice::from_ref(&root), Some("renamed"));
        assert!(matches!(result, Err(CollectError::NameNeedsOneFile)));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_folder_is_nothing_to_send() {
        let root = scratch("empty");
        let dir = root.join("empty");
        std::fs::create_dir_all(&dir).unwrap();

        let result = collect_targets(&[dir], None);
        assert!(matches!(result, Err(CollectError::Empty)));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_path_names_the_path() {
        let root = scratch("missing");
        let missing = root.join("nope.txt");

        match collect_targets(std::slice::from_ref(&missing), None) {
            Err(CollectError::Io { path, .. }) => assert_eq!(path, missing),
            other => panic!("expected an Io error, got {other:?}"),
        }

        std::fs::remove_dir_all(&root).ok();
    }
}
