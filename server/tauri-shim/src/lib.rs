// What fs.rs, tree.rs, search.rs and history.rs from src-tauri ask of Tauri,
// and nothing more: the attribute, and a handle that knows the data folder.
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

pub use tauri_shim_macros::command;

pub trait Runtime {}

pub struct Wry;
impl Runtime for Wry {}

pub struct AppHandle<R: Runtime = Wry> {
    data_dir: PathBuf,
    runtime: PhantomData<R>,
}

impl<R: Runtime> AppHandle<R> {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            runtime: PhantomData,
        }
    }

    /// What the server's `data_dir` gives history.rs.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}
