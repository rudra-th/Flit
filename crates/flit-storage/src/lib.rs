//! Safe paths and conflict naming.
//!
//! Everything here treats the sender as hostile. A manifest arriving from the
//! network may contain `..`, absolute paths, Windows device names, control
//! characters, or names that collapse to nothing on the receiving filesystem.
//! The receiver sanitizes *before* it opens anything, and joins are done one
//! segment at a time so a crafted name can never escape the destination root.
//!
//! This module is pure: no filesystem access, so the rules are testable without
//! a temp directory, and the same rules can be reused for Android SAF, where
//! there is no `Path` at all.

use std::path::{Path, PathBuf};

/// Longest file-name segment accepted, in bytes. Matches the common
/// filesystem limit; Android and Windows both reject more.
pub const MAX_SEGMENT_BYTES: usize = 255;

/// Names that Windows treats as devices regardless of extension.
const RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows forbids in a file name, plus the Unix separator.
const FORBIDDEN: [char; 10] = ['/', '\\', ':', '<', '>', '"', '|', '?', '*', '\0'];

/// Why a name or path was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The value was empty or contained no usable segment.
    #[error("empty path")]
    Empty,
    /// The value was absolute, which is never allowed for a received item.
    #[error("absolute paths are not accepted")]
    Absolute,
    /// A segment was `.` or `..`, which would escape the destination.
    #[error("path traversal segment: {0:?}")]
    Traversal(String),
    /// A segment contained a forbidden character.
    #[error("forbidden character {ch:?} in {segment:?}")]
    ForbiddenCharacter {
        /// The offending segment.
        segment: String,
        /// The offending character.
        ch: char,
    },
    /// A segment contained a control character.
    #[error("control character in {0:?}")]
    ControlCharacter(String),
    /// A segment ended with a dot or a space, which Windows silently strips.
    #[error("segment ends with a dot or space: {0:?}")]
    TrailingDotOrSpace(String),
    /// A segment was a Windows reserved device name.
    #[error("reserved device name: {0:?}")]
    ReservedName(String),
    /// A segment was longer than [`MAX_SEGMENT_BYTES`].
    #[error("segment is {len} bytes, limit is {max}")]
    TooLong {
        /// Actual length.
        len: usize,
        /// The limit.
        max: usize,
    },
}

/// Validate and return a single path segment.
///
/// The returned value is byte-identical to the input: this function rejects
/// hostile names rather than rewriting them, because a silently renamed
/// `report.pdf` is worse than a rejected one.
pub fn sanitize_segment(segment: &str) -> Result<&str, PathError> {
    if segment.is_empty() {
        return Err(PathError::Empty);
    }
    if segment == "." || segment == ".." {
        return Err(PathError::Traversal(segment.to_string()));
    }
    if segment.len() > MAX_SEGMENT_BYTES {
        return Err(PathError::TooLong {
            len: segment.len(),
            max: MAX_SEGMENT_BYTES,
        });
    }

    for ch in segment.chars() {
        if ch.is_control() {
            return Err(PathError::ControlCharacter(segment.to_string()));
        }
        if FORBIDDEN.contains(&ch) {
            return Err(PathError::ForbiddenCharacter {
                segment: segment.to_string(),
                ch,
            });
        }
    }

    if segment.ends_with('.') || segment.ends_with(' ') {
        return Err(PathError::TrailingDotOrSpace(segment.to_string()));
    }

    if is_reserved_name(segment) {
        return Err(PathError::ReservedName(segment.to_string()));
    }

    Ok(segment)
}

/// True when `name` is a Windows device name, with or without an extension.
///
/// `CON`, `con`, `CON.txt` and `Con.tar.gz` are all the same device.
#[must_use]
pub fn is_reserved_name(name: &str) -> bool {
    let stem = match name.find('.') {
        Some(idx) => &name[..idx],
        None => name,
    };
    let upper = stem.to_ascii_uppercase();
    RESERVED_NAMES.contains(&upper.as_str())
}

/// Split a received relative path into validated segments.
///
/// Both separators are accepted because the sender may be on any platform.
/// Empty segments from repeated or trailing separators are collapsed; leading
/// separators are refused, because an absolute path is never a legitimate
/// relative name.
pub fn sanitize_relative(relative: &str) -> Result<Vec<&str>, PathError> {
    if relative.starts_with('/') || relative.starts_with('\\') {
        return Err(PathError::Absolute);
    }

    let mut segments = Vec::new();
    for raw in relative.split(['/', '\\']) {
        if raw.is_empty() {
            continue;
        }
        segments.push(sanitize_segment(raw)?);
    }

    if segments.is_empty() {
        return Err(PathError::Empty);
    }
    Ok(segments)
}

/// Join a received relative path onto a destination root, one segment at a time.
///
/// The result is always a descendant of `root`: traversal is refused rather
/// than normalized away.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, PathError> {
    let segments = sanitize_relative(relative)?;
    let mut out = root.to_path_buf();
    for segment in segments {
        out.push(segment);
    }
    Ok(out)
}

/// Build the `n`th conflict-free name, matching the convention users expect.
///
/// `attempt` starts at 2: `report.pdf` becomes `report (2).pdf`, then
/// `report (3).pdf`. Values below 2 return the original name.
///
/// A leading-dot name with no other dot (`.env`) keeps its dot and gains the
/// counter after it: `.env (2)`.
#[must_use]
pub fn conflict_name(name: &str, attempt: u32) -> String {
    if attempt < 2 {
        return name.to_string();
    }

    let (stem, ext) = split_extension(name);
    if ext.is_empty() {
        format!("{stem} ({attempt})")
    } else {
        format!("{stem} ({attempt}).{ext}")
    }
}

/// Split `name` into stem and extension the way a user would read it.
///
/// `archive.tar.gz` splits as `("archive.tar", "gz")`; `.env` splits as
/// `(".env", "")`; `noext` splits as `("noext", "")`.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        // Index 0 means a dotfile with no extension.
        Some(idx) if idx > 0 => (&name[..idx], &name[idx + 1..]),
        _ => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["report.pdf", ".env", "a", "ünïcode.txt", "with space.txt"] {
            assert_eq!(sanitize_segment(name), Ok(name), "{name}");
        }
    }

    #[test]
    fn rejects_traversal_and_empties() {
        assert_eq!(
            sanitize_segment(".."),
            Err(PathError::Traversal("..".into()))
        );
        assert_eq!(sanitize_segment("."), Err(PathError::Traversal(".".into())));
        assert_eq!(sanitize_segment(""), Err(PathError::Empty));
    }

    #[test]
    fn rejects_separators_and_reserved_characters() {
        assert!(matches!(
            sanitize_segment("a/b"),
            Err(PathError::ForbiddenCharacter { ch: '/', .. })
        ));
        assert!(matches!(
            sanitize_segment("a\\b"),
            Err(PathError::ForbiddenCharacter { ch: '\\', .. })
        ));
        assert!(matches!(
            sanitize_segment("C:evil"),
            Err(PathError::ForbiddenCharacter { ch: ':', .. })
        ));
        assert!(matches!(
            sanitize_segment("what?"),
            Err(PathError::ForbiddenCharacter { ch: '?', .. })
        ));
    }

    #[test]
    fn rejects_control_characters() {
        assert!(matches!(
            sanitize_segment("null\0byte"),
            Err(PathError::ControlCharacter(_))
        ));
        assert!(matches!(
            sanitize_segment("bell\u{7}"),
            Err(PathError::ControlCharacter(_))
        ));
    }

    #[test]
    fn rejects_windows_device_names_in_any_case_or_extension() {
        for name in [
            "CON",
            "con",
            "Con.txt",
            "AUX.tar.gz",
            "COM1",
            "lpt9.log",
            "NUL",
        ] {
            assert!(
                matches!(sanitize_segment(name), Err(PathError::ReservedName(_))),
                "{name}"
            );
        }
        // Not reserved: CON2, CONSOLE.
        assert!(sanitize_segment("CON2").is_ok());
        assert!(sanitize_segment("CONSOLE.txt").is_ok());
    }

    #[test]
    fn rejects_trailing_dot_or_space() {
        assert!(matches!(
            sanitize_segment("name."),
            Err(PathError::TrailingDotOrSpace(_))
        ));
        assert!(matches!(
            sanitize_segment("name "),
            Err(PathError::TrailingDotOrSpace(_))
        ));
    }

    #[test]
    fn rejects_overlong_segments() {
        let long = "a".repeat(MAX_SEGMENT_BYTES + 1);
        assert!(matches!(
            sanitize_segment(&long),
            Err(PathError::TooLong { .. })
        ));
        assert!(sanitize_segment(&"a".repeat(MAX_SEGMENT_BYTES)).is_ok());
    }

    #[test]
    fn relative_paths_are_split_and_validated() {
        assert_eq!(
            sanitize_relative("dir/sub/file.txt"),
            Ok(vec!["dir", "sub", "file.txt"])
        );
        assert_eq!(
            sanitize_relative("dir\\sub\\file.txt"),
            Ok(vec!["dir", "sub", "file.txt"])
        );
        assert_eq!(
            sanitize_relative("dir//file.txt"),
            Ok(vec!["dir", "file.txt"])
        );
        assert_eq!(
            sanitize_relative("dir/../other"),
            Err(PathError::Traversal("..".into()))
        );
    }

    #[test]
    fn relative_paths_reject_absolute_input() {
        assert_eq!(sanitize_relative("/etc/passwd"), Err(PathError::Absolute));
        assert_eq!(
            sanitize_relative("\\\\server\\share"),
            Err(PathError::Absolute)
        );
    }

    #[test]
    fn joins_stay_inside_the_root() {
        let root = Path::new("/recv");
        assert_eq!(
            safe_join(root, "a/b.txt").unwrap(),
            root.join("a").join("b.txt")
        );
        assert!(safe_join(root, "../escape").is_err());
    }

    #[test]
    fn conflict_names_follow_the_expected_convention() {
        assert_eq!(conflict_name("report.pdf", 2), "report (2).pdf");
        assert_eq!(conflict_name("report.pdf", 3), "report (3).pdf");
        assert_eq!(conflict_name("noext", 2), "noext (2)");
        assert_eq!(conflict_name(".env", 2), ".env (2)");
        assert_eq!(conflict_name("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(conflict_name("report.pdf", 1), "report.pdf");
    }

    proptest::proptest! {
        /// Whatever survives sanitization must be safe to hand to a filesystem.
        #[test]
        fn sanitized_segments_are_always_safe(name in "\\PC*") {
            if let Ok(segment) = sanitize_segment(&name) {
                proptest::prop_assert!(!segment.is_empty());
                proptest::prop_assert_ne!(segment, ".");
                proptest::prop_assert_ne!(segment, "..");
                proptest::prop_assert!(!segment.contains('/'));
                proptest::prop_assert!(!segment.contains('\\'));
                proptest::prop_assert!(!segment.ends_with('.'));
                proptest::prop_assert!(!segment.ends_with(' '));
                proptest::prop_assert!(segment.len() <= MAX_SEGMENT_BYTES);
                proptest::prop_assert!(!is_reserved_name(segment));
            }
        }

        /// Conflict naming must always produce a different name, and keep any
        /// extension intact.
        #[test]
        fn conflict_names_never_repeat_and_keep_the_extension(stem in "[A-Za-z0-9]{1,20}", attempt in 2u32..100) {
            let original = format!("{stem}.txt");
            let renamed = conflict_name(&original, attempt);
            proptest::prop_assert_ne!(renamed.as_str(), original.as_str());
            proptest::prop_assert!(renamed.ends_with(".txt"));
            proptest::prop_assert!(renamed.starts_with(&stem));
        }
    }
}
