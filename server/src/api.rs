// `invoke` over HTTP: the same commands, arguments and answers as Tauri's,
// with paths translated at the door (spec §2.2).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::{Root, Roots, DATA, LIBRARY};
use crate::{fs, history, search, trashcan, tree};

const ANY: &[Root] = &[Root::Library, Root::Data];
const LIB: &[Root] = &[Root::Library];
const OWN: &[Root] = &[Root::Data];

type Answer = Result<Value, (StatusCode, Value)>;

fn refused(message: String) -> (StatusCode, Value) {
    (StatusCode::BAD_REQUEST, Value::String(message))
}

/// Tauri hands `Err` to the front end as the rejection value, as it is.
fn reply<T: Serialize, E: Serialize>(result: Result<T, E>) -> Answer {
    match result {
        Ok(value) => Ok(json!(value)),
        Err(error) => Err((StatusCode::BAD_REQUEST, json!(error))),
    }
}

/// The server's own secrets in `/data` — the sign-in and the sessions —
/// are never read back out to a page, signed in or not.
fn not_secret(args: &Value) -> Result<(), (StatusCode, Value)> {
    const SECRETS: [&str; 2] = ["auth.json", "sessions.json"];
    let path = args["path"].as_str().unwrap_or("").trim_end_matches('/');
    match path.strip_prefix("/data/") {
        Some(name) if SECRETS.contains(&name) => Err(refused(format!("not allowed: {path}"))),
        _ => Ok(()),
    }
}

/// Replaces the virtual path at `pointer` with the real one, in place.
fn real(
    roots: &Roots,
    args: &mut Value,
    pointer: &str,
    allowed: &[Root],
) -> Result<(), (StatusCode, Value)> {
    let slot = args
        .pointer_mut(pointer)
        .ok_or_else(|| refused(format!("missing {pointer}")))?;
    let path = slot
        .as_str()
        .ok_or_else(|| refused(format!("bad {pointer}")))?
        .to_string();
    *slot = Value::String(
        roots
            .to_real(&path, allowed)
            .map_err(refused)?
            .to_string_lossy()
            .into_owned(),
    );
    Ok(())
}

/// The two files of ours the page writes on the server: settings, shared by
/// every device, and the session. Drafts stay in the browser (web/drafts.ts),
/// and the history and the trash are the server's alone to write.
const OWN_FILES: &[&str] = &["/data/settings.json", "/data/state.json"];

fn own(roots: &Roots, args: &mut Value) -> Result<(), (StatusCode, Value)> {
    let path = args["path"].as_str().unwrap_or_default();
    if !OWN_FILES.contains(&path) {
        return Err(refused(format!("not allowed: {path}")));
    }
    real(roots, args, "/path", OWN)
}

/// Where a write, a new name or the trash may reach: inside the library,
/// never the library itself (that would take every document with it), and
/// never a dot-folder the tree does not show — `.git/hooks` runs code.
/// By name: a symlink already in the library that points into one is not
/// seen, and only someone who can write to the folder could have made it.
pub fn writable(roots: &Roots, path: &str) -> Result<std::path::PathBuf, String> {
    let rest = path
        .strip_prefix(LIBRARY)
        .unwrap_or_default()
        .trim_matches('/');
    if rest.is_empty() || rest.split('/').any(|part| part.starts_with('.')) {
        return Err(format!("not allowed: {path}"));
    }
    roots.to_real(path, LIB)
}

/// `real`, for a path that is about to change (`writable`).
fn changing(roots: &Roots, args: &mut Value, pointer: &str) -> Result<(), (StatusCode, Value)> {
    let path = args
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default();
    writable(roots, path).map_err(refused)?;
    real(roots, args, pointer, LIB)
}

fn arg<T: for<'de> Deserialize<'de>>(args: &Value, key: &str) -> Result<T, (StatusCode, Value)> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|error| refused(format!("{key}: {error}")))
}

/// What the shared modules take to find the data folder (main.rs `data_dir`).
fn app(roots: &Roots) -> tauri::AppHandle {
    tauri::AppHandle::new(roots.data())
}

pub fn dispatch(roots: &Roots, command: &str, mut args: Value) -> Answer {
    let a = &mut args;
    let answer = match command {
        "read_file" => {
            not_secret(a)?;
            real(roots, a, "/path", ANY)?;
            reply(fs::read_file(arg(a, "path")?, arg(a, "encoding")?))
        }
        "hash_file" => {
            not_secret(a)?;
            real(roots, a, "/path", ANY)?;
            reply(fs::hash_file(arg(a, "path")?))
        }
        "canonical_path" => {
            real(roots, a, "/path", ANY)?;
            Ok(json!(fs::canonical_path(arg(a, "path")?)))
        }
        "read_tree" => {
            real(roots, a, "/root", LIB)?;
            reply(tree::read_tree(arg(a, "root")?, arg(a, "extensions")?))
        }
        "path_kind" => {
            real(roots, a, "/path", LIB)?;
            Ok(json!(tree::path_kind(arg(a, "path")?)))
        }
        "search_folder" => {
            real(roots, a, "/request/root", LIB)?;
            reply(search::search_folder(arg(a, "request")?))
        }
        "collect_tags" => {
            real(roots, a, "/root", LIB)?;
            reply(search::collect_tags(arg(a, "root")?, arg(a, "extensions")?))
        }
        "backlinks" => {
            real(roots, a, "/root", LIB)?;
            real(roots, a, "/path", LIB)?;
            reply(search::backlinks(
                arg(a, "root")?,
                arg(a, "extensions")?,
                arg(a, "path")?,
            ))
        }
        "write_text_atomic" => {
            own(roots, a)?;
            reply(fs::write_text_atomic(arg(a, "path")?, arg(a, "text")?))
        }
        "write_file_atomic" => {
            changing(roots, a, "/request/path")?;
            write_document(roots, arg(a, "request")?)
        }
        "snapshot_text" => reply(history::snapshot_text(app(roots), arg(a, "request")?)),
        "list_snapshots" => reply(history::list_snapshots(app(roots), arg(a, "id")?)),
        "rename_history" => reply(history::rename_history(
            app(roots),
            arg(a, "oldId")?,
            arg(a, "newId")?,
        )),
        "delete_snapshot" => {
            real(roots, a, "/path", OWN)?;
            reply(history::delete_snapshot(app(roots), arg(a, "path")?))
        }
        "rename_path" => {
            changing(roots, a, "/from")?;
            changing(roots, a, "/to")?;
            reply(tree::rename_path(arg(a, "from")?, arg(a, "to")?))
        }
        "create_file" => {
            changing(roots, a, "/path")?;
            reply(tree::create_file(arg(a, "path")?))
        }
        "create_dir" => {
            changing(roots, a, "/path")?;
            reply(tree::create_dir(arg(a, "path")?))
        }
        // The server's own trash, not the desktop's Recycle Bin (trashcan.rs).
        "trash_path" => {
            changing(roots, a, "/path")?;
            reply(trashcan::put(roots, &arg::<String>(a, "path")?))
        }
        "trash_list" => Ok(json!(trashcan::list(roots))),
        "trash_restore" => reply(trashcan::restore(roots, &arg::<String>(a, "id")?)),
        "data_path" => Ok(json!(DATA)),
        "take_opening" => Ok(take_opening(roots)),
        // Outside the roots is "not there", as a missing link target is on
        // the desktop (read/dom.ts).
        "fs_stat" => match real(roots, a, "/path", ANY) {
            Ok(()) => Ok(stat(&arg::<String>(a, "path")?)),
            Err(_) => Ok(Value::Null),
        },
        "fs_read_dir" => {
            real(roots, a, "/path", ANY)?;
            read_dir(&arg::<String>(a, "path")?).map_err(|e| (StatusCode::BAD_REQUEST, json!(e)))
        }
        _ => Err((
            StatusCode::NOT_FOUND,
            json!(format!("not available: {command}")),
        )),
    };
    match answer {
        Ok(mut value) => {
            roots.virtualize(&mut value);
            if let (Value::String(path), "canonical_path") = (&value, command) {
                value = json!(roots.to_virtual(path).unwrap_or_default());
            }
            Ok(value)
        }
        Err((status, mut value)) => {
            roots.scrub(&mut value);
            Err((status, value))
        }
    }
}

/// The desktop's write, with two differences. A request without a base hash
/// is Save As on the desktop and overwrites; there is no Save As here, so
/// it may only create. And the same bytes already on disk are a success: a
/// save whose answer was lost on the way is sent again (web/core.ts), and
/// its old base hash would otherwise read as a conflict with itself.
fn write_document(roots: &Roots, request: fs::WriteRequest) -> Answer {
    if let (Ok(bytes), Ok(Some(current))) = (
        fs::encoded_bytes(&request),
        fs::hash_file(request.path.clone()),
    ) {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if hash == current {
            return Ok(json!({ "hash": hash, "snapshotError": null }));
        }
    }
    if request.base_hash.is_none() && std::path::Path::new(&request.path).exists() {
        let conflict = json!({ "kind": "conflict", "message": "file changed on disk" });
        return Err((StatusCode::BAD_REQUEST, conflict));
    }
    reply(fs::write_file_atomic(app(roots), request))
}

/// The library opens itself on the first run, and whenever the session
/// names something else: there is no folder dialog to get back with. Any
/// other time the session restores it, with its open files (bootstrap.ts).
fn take_opening(roots: &Roots) -> Value {
    let library = std::fs::read_to_string(roots.data().join("state.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|state| {
            state
                .get("library")
                .and_then(Value::as_str)
                .map(String::from)
        });
    let folders: &[&str] = if library.as_deref() == Some(LIBRARY) {
        &[]
    } else {
        &[LIBRARY]
    };
    json!({ "paths": [], "folders": folders, "seed": null })
}

/// What plugin-fs `stat` gives, the fields Plain reads; `null` when missing.
fn stat(path: &str) -> Value {
    let Ok(meta) = std::fs::metadata(path) else {
        return Value::Null;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    json!({ "isFile": meta.is_file(), "isDirectory": meta.is_dir(), "size": meta.len(), "mtime": mtime })
}

fn read_dir(path: &str) -> Result<Value, String> {
    let entries = std::fs::read_dir(path).map_err(|e| e.to_string())?;
    let list: Vec<Value> = entries
        .flatten()
        .map(|entry| {
            let kind = entry.file_type().ok();
            json!({
                "name": entry.file_name().to_string_lossy(),
                "isFile": kind.is_some_and(|k| k.is_file()),
                "isDirectory": kind.is_some_and(|k| k.is_dir()),
            })
        })
        .collect();
    Ok(json!(list))
}

/* ----------------------------------------------------------------- http */

pub async fn invoke(
    State(roots): State<Arc<Roots>>,
    UrlPath(command): UrlPath<String>,
    body: Bytes,
) -> Response {
    let args = if body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(args) => args,
            Err(error) => {
                return (StatusCode::BAD_REQUEST, Json(json!(error.to_string()))).into_response()
            }
        }
    };
    let answer = tokio::task::spawn_blocking(move || dispatch(&roots, &command, args)).await;
    match answer {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err((status, value))) => (status, Json(value)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Deserialize)]
pub struct AssetQuery {
    path: String,
}

/// A file from the library is shown, never run: an `.html` or `.svg` there
/// would otherwise script this origin and its API.
const ASSET_CSP: &str =
    "sandbox; default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'";

/// Images and links inside the library (spec §2.2), and only there.
pub async fn asset(State(roots): State<Arc<Roots>>, Query(query): Query<AssetQuery>) -> Response {
    let read = move || {
        let real = roots
            .to_real(&query.path, LIB)
            .map_err(|_| StatusCode::FORBIDDEN)?;
        let bytes = std::fs::read(&real).map_err(|_| StatusCode::NOT_FOUND)?;
        Ok::<_, StatusCode>((mime_guess::from_path(&real).first_or_octet_stream(), bytes))
    };
    match tokio::task::spawn_blocking(read).await {
        Ok(Ok((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (header::CONTENT_SECURITY_POLICY, ASSET_CSP.to_string()),
            ],
            bytes,
        )
            .into_response(),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A pasted or dropped picture, into the library (editor/images.ts picks a
/// free name first). Never over an existing file.
pub async fn upload(
    State(roots): State<Arc<Roots>>,
    Query(query): Query<AssetQuery>,
    body: Bytes,
) -> Response {
    let write = move || {
        let real = writable(&roots, &query.path).map_err(refused)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&real)
            .map_err(|e| match e.kind() {
                // As `create_file` says it, so the page can take the next name.
                std::io::ErrorKind::AlreadyExists => (
                    StatusCode::BAD_REQUEST,
                    json!({ "kind": "exists", "message": format!("{} exists", query.path) }),
                ),
                _ => refused(e.to_string()),
            })?;
        std::io::Write::write_all(&mut file, &body).map_err(|e| {
            let _ = std::fs::remove_file(&real);
            refused(e.to_string())
        })
    };
    match tokio::task::spawn_blocking(write).await {
        Ok(Ok(())) => Json(Value::Null).into_response(),
        Ok(Err((status, value))) => (status, Json(value)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
