//! Turning dropped or picked paths into the files to queue: folders are expanded.

use std::path::{Path, PathBuf};

/// Most files one drop can add, so dropping a huge folder (a home directory) by mistake
/// doesn't flood the queue.
pub const MAX_FILES: usize = 1000;

/// Supported files among `paths`, in the given order; folders contribute their supported
/// files, subfolders included, sorted by path. Hidden entries inside folders and symlinked
/// folders (which can loop) are skipped. Returns the files and whether `limit` cut it short.
pub fn expand(paths: &[PathBuf], limit: usize) -> (Vec<PathBuf>, bool) {
    // One past the limit, to tell a cut list from one that is exactly full.
    let cap = limit + 1;
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, cap, &mut out);
        } else if crate::is_supported(path) {
            out.push(path.clone());
        }
        if out.len() >= cap {
            break;
        }
    }
    let cut = out.len() > limit;
    out.truncate(limit);
    (out, cut)
}

fn walk(dir: &Path, limit: usize, out: &mut Vec<PathBuf>) {
    // shortcut: unreadable folders are skipped silently; surface it once there is a notice area.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        // file_type doesn't follow symlinks, so a symlinked folder is neither walked nor added.
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, limit, out),
            Ok(_) if crate::is_supported(&path) => out.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A fresh, empty directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fanhuaji-gpui-files-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x").unwrap();
    }

    #[test]
    fn keeps_supported_files_in_order() {
        let d = scratch("plain");
        let (b, a, png) = (d.join("b.srt"), d.join("a.txt"), d.join("c.png"));
        for f in [&b, &a, &png] {
            touch(f);
        }
        assert_eq!(
            expand(&[b.clone(), png, a.clone()], MAX_FILES),
            (vec![b, a], false)
        );
    }

    #[test]
    fn expands_folders_recursively_and_sorted() {
        let d = scratch("tree");
        for f in [
            "s2/e1.srt",
            "s1/e2.ass",
            "s1/e1.srt",
            "notes.md",
            "cover.jpg",
        ] {
            touch(&d.join(f));
        }
        let (files, cut) = expand(std::slice::from_ref(&d), MAX_FILES);
        let rel: Vec<_> = files
            .iter()
            .map(|f| f.strip_prefix(&d).unwrap().to_owned())
            .collect();
        assert_eq!(
            rel,
            ["notes.md", "s1/e1.srt", "s1/e2.ass", "s2/e1.srt"].map(PathBuf::from)
        );
        assert!(!cut);
    }

    #[test]
    fn skips_hidden_entries_inside_folders() {
        let d = scratch("hidden");
        for f in [".git/x.txt", ".DS_Store.txt", "keep.srt"] {
            touch(&d.join(f));
        }
        assert_eq!(
            expand(std::slice::from_ref(&d), MAX_FILES).0,
            vec![d.join("keep.srt")]
        );
    }

    #[test]
    fn stops_at_the_limit() {
        let d = scratch("limit");
        for i in 0..5 {
            touch(&d.join(format!("{i}.txt")));
        }
        let (files, cut) = expand(std::slice::from_ref(&d), 3);
        assert_eq!(files.len(), 3);
        assert!(cut);
        assert_eq!(
            expand(std::slice::from_ref(&d), 5),
            (expand(std::slice::from_ref(&d), 9).0, false),
            "exactly the limit is not cut"
        );
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinked_folders() {
        let d = scratch("symlink");
        touch(&d.join("real/a.srt"));
        std::os::unix::fs::symlink(&d, d.join("real/loop")).unwrap();
        assert_eq!(
            expand(std::slice::from_ref(&d), MAX_FILES).0,
            vec![d.join("real/a.srt")]
        );
    }
}
