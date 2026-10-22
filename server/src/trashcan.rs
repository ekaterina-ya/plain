// The server's trash (ТЗ-Plain-Web §9.2). A browser has no Recycle Bin to
// hand a file to, so `delete` moves it to data/trash, where it waits thirty
// days and can be put back from the menu (app/trash.ts).
//
// data/trash/<id> is the file or folder itself, data/trash/<id>.json says
// where it came from.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths::Roots;

pub const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The note's file name is what counts (`read`).
    #[serde(default)]
    id: String,
    /// Virtual, so the note stays true when the library moves.
    path: String,
    deleted_at: f64,
    dir: bool,
}

fn folder(roots: &Roots) -> PathBuf {
    roots.data().join("trash")
}

fn millis(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as f64)
        .unwrap_or(0.0)
}

/// A rename, or a copy where the library and the data folder are on two
/// disks (two volumes of a container, say), which a rename cannot cross.
/// A symlink stays a symlink: copied as what it points to, it would bring
/// a file from outside the library in on the way back.
fn relocate(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(error) = copy(from, to) {
                let _ = remove(to);
                return Err(error);
            }
            remove(from)
        }
        moved => moved,
    }
}

fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    if meta.is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
        #[cfg(not(unix))]
        return Err(io::Error::other("a symlink cannot cross disks here"));
    }
    if !meta.is_dir() {
        return fs::copy(from, to).map(|_| ());
    }
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        copy(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(_) => Ok(()),
    }
}

/// `real` is already checked to be inside the library (api.rs).
pub fn put(roots: &Roots, real: &str) -> Result<(), String> {
    let path = roots.to_virtual(real).ok_or("not in the library")?;
    let dir = fs::symlink_metadata(real)
        .map_err(|e| e.to_string())?
        .is_dir();
    let trash = folder(roots);
    fs::create_dir_all(&trash).map_err(|e| e.to_string())?;
    let now = SystemTime::now();
    let stamp = crate::history::stamp(now);
    let id = (0..)
        .map(|n| {
            if n == 0 {
                stamp.clone()
            } else {
                format!("{stamp}-{n}")
            }
        })
        .find(|id| !trash.join(id).exists() && !trash.join(format!("{id}.json")).exists())
        .unwrap_or(stamp);
    let note = trash.join(format!("{id}.json"));
    let entry = Entry {
        id: id.clone(),
        path,
        deleted_at: millis(now),
        dir,
    };
    // The note first: a file in the trash that nobody knows the way back for
    // is as good as deleted.
    fs::write(&note, serde_json::to_string(&entry).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    if let Err(error) = relocate(Path::new(real), &trash.join(&id)) {
        let _ = fs::remove_file(&note);
        return Err(error.to_string());
    }
    Ok(())
}

fn read(roots: &Roots, id: &str) -> Option<Entry> {
    let text = fs::read_to_string(folder(roots).join(format!("{id}.json"))).ok()?;
    let entry: Entry = serde_json::from_str(&text).ok()?;
    Some(Entry {
        id: id.to_string(),
        ..entry
    })
}

fn notes(roots: &Roots) -> Vec<Entry> {
    let Ok(names) = fs::read_dir(folder(roots)) else {
        return Vec::new();
    };
    names
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(String::from)
        })
        .filter_map(|id| read(roots, &id))
        .collect()
}

/// Newest first; a note whose item has gone is not listed.
pub fn list(roots: &Roots) -> Vec<Entry> {
    let mut found: Vec<Entry> = notes(roots)
        .into_iter()
        .filter(|entry| folder(roots).join(&entry.id).symlink_metadata().is_ok())
        .collect();
    found.sort_by(|a, b| b.deleted_at.total_cmp(&a.deleted_at));
    found
}

/// Back where it was. A name taken since is not overwritten: the user is
/// told, and both stay as they are. The note is checked like any other write
/// (api.rs `writable`): whatever it says, nothing lands in a dot-folder.
pub fn restore(roots: &Roots, id: &str) -> Result<String, String> {
    let gone = || "no longer in the trash".to_string();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(gone());
    }
    let entry = read(roots, id).ok_or_else(gone)?;
    let item = folder(roots).join(id);
    if item.symlink_metadata().is_err() {
        return Err(gone());
    }
    let target = crate::api::writable(roots, &entry.path)?;
    if target.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", entry.path));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    relocate(&item, &target).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(folder(roots).join(format!("{id}.json")));
    Ok(entry.path)
}

/// Thirty days, at startup and once a day after (main.rs).
pub fn sweep(roots: &Roots, now: SystemTime) {
    for entry in notes(roots) {
        if millis(now) - entry.deleted_at < KEEP.as_millis() as f64 {
            continue;
        }
        if remove(&folder(roots).join(&entry.id)).is_ok() {
            let _ = fs::remove_file(folder(roots).join(format!("{}.json", entry.id)));
        }
    }
}
