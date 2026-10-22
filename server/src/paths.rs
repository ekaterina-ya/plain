// The browser never sees a server path (spec §2.5). It works with two
// virtual roots, `/library` and `/data`, spelled like any absolute path so
// the front end's path code keeps working; this is where they turn into real
// ones and back.

use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Root {
    Library,
    Data,
}

pub const LIBRARY: &str = "/library";
pub const DATA: &str = "/data";

pub struct Roots {
    library: PathBuf,
    data: PathBuf,
}

impl Roots {
    /// Canonical once, here: `read_file` and `backlinks` hand back canonical
    /// paths, and on macOS `/var/…` comes back as `/private/var/…`.
    /// One inside the other would let a write to `/data` reach a document.
    pub fn new(library: &Path, data: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(data)?;
        let (library, data) = (library.canonicalize()?, data.canonicalize()?);
        if library.starts_with(&data) || data.starts_with(&library) {
            return Err(std::io::Error::other(
                "the library and the data folder must not overlap",
            ));
        }
        Ok(Self { library, data })
    }

    pub fn data(&self) -> &Path {
        &self.data
    }

    pub fn library(&self) -> &Path {
        &self.library
    }

    fn pairs(&self) -> [(Root, &'static str, &Path); 2] {
        [
            (Root::Library, LIBRARY, &self.library),
            (Root::Data, DATA, &self.data),
        ]
    }

    /// A virtual path in one of `allowed` to the real one. Refuses anything
    /// that names a way out: `..`, a backslash, a NUL, or a symlink that
    /// lands outside the root. The answer is checked, not canonical: a
    /// symlink swapped in between the check and the use would get through,
    /// which only someone who can already write to the folder could do.
    pub fn to_real(&self, path: &str, allowed: &[Root]) -> Result<PathBuf, String> {
        let refuse = || format!("not allowed: {path}");
        let (root, rest) = self
            .pairs()
            .into_iter()
            .filter(|(root, _, _)| allowed.contains(root))
            .find_map(|(_, name, real)| match path.strip_prefix(name) {
                Some("") => Some((real, "")),
                Some(rest) if rest.starts_with('/') => Some((real, rest)),
                _ => None,
            })
            .ok_or_else(refuse)?;
        let mut real = root.to_path_buf();
        for part in rest.trim_end_matches('/').split('/').skip(1) {
            if matches!(part, "" | "." | "..") || part.contains(['\\', '\0']) {
                return Err(refuse());
            }
            real.push(part);
        }
        // The nearest part that exists decides where this really is.
        let existing = real.ancestors().find(|p| p.symlink_metadata().is_ok());
        match existing.map(Path::canonicalize) {
            Some(Ok(found)) if found.starts_with(root) => Ok(real),
            _ => Err(refuse()),
        }
    }

    pub fn to_virtual(&self, real: &str) -> Option<String> {
        self.pairs().into_iter().find_map(|(_, name, root)| {
            let root = root.to_str()?;
            match real.strip_prefix(root) {
                Some("") => Some(name.to_string()),
                Some(rest) if rest.starts_with('/') => Some(format!("{name}{rest}")),
                _ => None,
            }
        })
    }

    /// Every `"path"` in an answer — tree nodes, files, search hits,
    /// backlinks — and the `"files"` of a tag, and nothing else: a document's
    /// text may well start with the root and must come back as it is.
    pub fn virtualize(&self, value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, item) in map.iter_mut() {
                    match (key.as_str(), item) {
                        ("path", Value::String(text)) => self.swap(text),
                        ("files", Value::Array(items)) if items.iter().all(Value::is_string) => {
                            items.iter_mut().for_each(|item| {
                                if let Value::String(text) = item {
                                    self.swap(text);
                                }
                            });
                        }
                        (_, item) => self.virtualize(item),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.virtualize(item)),
            _ => {}
        }
    }

    fn swap(&self, text: &mut String) {
        if let Some(path) = self.to_virtual(text) {
            *text = path;
        }
    }

    /// Error messages name paths in passing (`not a folder: …`, `… is gone`).
    pub fn scrub(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                for (_, name, root) in self.pairs() {
                    let Some(root) = root.to_str() else { continue };
                    let pattern =
                        regex::Regex::new(&format!(r#"{}([/\s'"]|$)"#, regex::escape(root)));
                    if let Ok(pattern) = pattern {
                        *text = pattern.replace_all(text, format!("{name}$1")).into_owned();
                    }
                }
            }
            Value::Object(map) => map.values_mut().for_each(|item| self.scrub(item)),
            Value::Array(items) => items.iter_mut().for_each(|item| self.scrub(item)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn setup() -> (tempfile::TempDir, Roots) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("lib/notes")).unwrap();
        fs::write(dir.path().join("lib/notes/a.md"), "a").unwrap();
        fs::write(dir.path().join("secret.md"), "s").unwrap();
        let roots = Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap();
        (dir, roots)
    }

    const ANY: &[Root] = &[Root::Library, Root::Data];

    #[test]
    fn inside_the_roots_is_allowed() {
        let (_dir, roots) = setup();
        let real = roots.to_real("/library/notes/a.md", ANY).unwrap();
        assert!(real.ends_with("lib/notes/a.md"));
        assert_eq!(roots.to_real("/library", ANY).unwrap(), roots.library);
        assert_eq!(roots.to_real("/library/", ANY).unwrap(), roots.library);
        // Not there yet, several levels down: a snapshot about to be written.
        let snapshot = roots.to_real("/data/history/later/x.md", ANY).unwrap();
        assert!(snapshot.starts_with(roots.data()));
    }

    #[test]
    fn ways_out_are_refused() {
        let (_dir, roots) = setup();
        for path in [
            "/library/../secret.md",
            "/library/notes/../../secret.md",
            "/library/./notes/a.md",
            "/library//notes",
            "/library2/a.md",
            "/etc/passwd",
            "library/notes",
            "/library/notes\\..\\..\\secret.md",
            "/library/a\0b",
            "",
        ] {
            assert!(roots.to_real(path, ANY).is_err(), "{path:?}");
        }
    }

    #[test]
    fn a_root_not_allowed_is_refused() {
        let (_dir, roots) = setup();
        assert!(roots.to_real("/library/notes/a.md", &[Root::Data]).is_err());
        assert!(roots.to_real("/data/state.json", &[Root::Library]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_counts_where_it_lands() {
        let (dir, roots) = setup();
        let lib = dir.path().join("lib");
        std::os::unix::fs::symlink(dir.path().join("secret.md"), lib.join("out.md")).unwrap();
        std::os::unix::fs::symlink(dir.path(), lib.join("outdir")).unwrap();
        std::os::unix::fs::symlink(lib.join("notes/a.md"), lib.join("in.md")).unwrap();
        assert!(roots.to_real("/library/out.md", ANY).is_err());
        assert!(roots.to_real("/library/outdir/secret.md", ANY).is_err());
        assert!(roots.to_real("/library/outdir/new.md", ANY).is_err());
        assert!(roots.to_real("/library/in.md", ANY).is_ok());
    }

    #[test]
    fn roots_are_canonical() {
        let (dir, roots) = setup();
        // A tempdir on macOS is under /var, which is /private/var.
        let canonical = dir.path().canonicalize().unwrap().join("lib/notes/a.md");
        assert_eq!(
            roots.to_virtual(canonical.to_str().unwrap()).as_deref(),
            Some("/library/notes/a.md")
        );
    }

    #[test]
    fn only_paths_are_virtualized() {
        let (_dir, roots) = setup();
        let lib = roots.library.to_str().unwrap().to_string();
        let mut answer = json!([{
            "path": format!("{lib}/notes"),
            "text": format!("{lib}/notes is where it is"),
            "children": [{ "path": format!("{lib}/notes/a.md") }],
        }, { "path": format!("{lib}2/x.md") }]);
        roots.virtualize(&mut answer);
        assert_eq!(answer[0]["path"], "/library/notes");
        assert_eq!(answer[0]["text"], format!("{lib}/notes is where it is"));
        assert_eq!(answer[0]["children"][0]["path"], "/library/notes/a.md");
        assert_eq!(answer[1]["path"], format!("{lib}2/x.md"));
    }

    #[test]
    fn tag_files_are_virtualized() {
        let (_dir, roots) = setup();
        let lib = roots.library.to_str().unwrap().to_string();
        let mut tags = json!([{ "tag": "x", "count": 1, "files": [format!("{lib}/notes/a.md")] }]);
        roots.virtualize(&mut tags);
        assert_eq!(tags[0]["files"][0], "/library/notes/a.md");
    }

    #[test]
    fn errors_lose_the_real_path() {
        let (_dir, roots) = setup();
        let lib = roots.library.to_str().unwrap().to_string();
        let mut error = json!({
            "kind": "missing",
            "message": format!("{lib}/a.md is gone, {lib} too; {lib}2 is not ours"),
        });
        roots.scrub(&mut error);
        assert_eq!(
            error["message"],
            format!("/library/a.md is gone, /library too; {lib}2 is not ours")
        );
    }

    #[test]
    fn overlapping_roots_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("lib")).unwrap();
        assert!(Roots::new(&dir.path().join("lib"), &dir.path().join("lib/data")).is_err());
        assert!(Roots::new(&dir.path().join("lib"), dir.path()).is_err());
        assert!(Roots::new(&dir.path().join("lib"), &dir.path().join("lib")).is_err());
    }
}
