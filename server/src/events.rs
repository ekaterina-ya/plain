// Changes in the library, to every open tab (ТЗ-Plain-Web §9.2). The
// desktop's watch.rs is a watcher per window and speaks Tauri; here one
// watcher covers the library and a broadcast reaches every socket. What a
// change means is still the front end's to decide (app/watcher.ts).
//
// A tab may also say something to the others — settings it has just
// written (web/link.ts). That goes to everyone but the one who said it.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use serde_json::json;
use tokio::sync::broadcast;

use crate::auth::Signed;
use crate::paths::Roots;

pub type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Who said it (0 is the library itself) and what.
pub struct Hub {
    sender: broadcast::Sender<(u64, String)>,
    next: AtomicU64,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sender: broadcast::channel(256).0,
            next: AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    pub fn subscribe(&self) -> broadcast::Receiver<(u64, String)> {
        self.sender.subscribe()
    }
}

/// `.git`, `.obsidian`, `node_modules`, and the `.tmp…` our own atomic write
/// leaves for a moment — as watch.rs has it.
fn ignored(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name == "node_modules" || (name.starts_with('.') && name != "." && name != "..")
    })
}

/// The library, recursively, debounced as on the desktop. Dropping the
/// answer stops it.
pub fn watch(roots: Arc<Roots>, hub: Arc<Hub>) -> Result<Watcher, String> {
    let library = roots.library().to_path_buf();
    let watched = library.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        None,
        move |result: DebounceEventResult| {
            let Ok(events) = result else { return };
            let mut paths: Vec<String> = Vec::new();
            for path in events.iter().flat_map(|event| &event.paths) {
                // Inside the library only: the library itself may well live
                // under a dot-folder on the server.
                let inside = path.strip_prefix(&library).unwrap_or(path);
                let Some(path) = path.to_str().filter(|_| !ignored(inside)) else {
                    continue;
                };
                if let Some(path) = roots.to_virtual(path).filter(|p| !paths.contains(p)) {
                    paths.push(path);
                }
            }
            if !paths.is_empty() {
                let message = json!({ "event": "fs-change", "payload": { "paths": paths } });
                let _ = hub.sender.send((0, message.to_string()));
            }
        },
    )
    .map_err(|error| error.to_string())?;
    debouncer
        .watch(&watched, RecursiveMode::Recursive)
        .map_err(|error| error.to_string())?;
    Ok(debouncer)
}

/// A page from another site may open a socket to this one, and would hear
/// every file name that changes; the browser always says where it is from.
pub fn same_origin(headers: &HeaderMap) -> bool {
    let value = |name| headers.get(name).and_then(|v| v.to_str().ok());
    match (value(header::ORIGIN), value(header::HOST)) {
        (Some(origin), Some(host)) => origin.split_once("://").map(|(_, rest)| rest) == Some(host),
        _ => false,
    }
}

pub async fn socket(
    State(hub): State<Arc<Hub>>,
    Extension(signed): Extension<Signed>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade
        .max_message_size(64 << 10)
        .on_upgrade(move |socket| serve(hub, signed, socket))
}

async fn serve(hub: Arc<Hub>, signed: Signed, mut socket: WebSocket) {
    let me = hub.next.fetch_add(1, Ordering::Relaxed);
    let mut heard = hub.sender.subscribe();
    // A device signed out elsewhere stops hearing at once; its reconnect
    // then meets the 401 (auth.rs `guard`).
    let mut revoked = signed.auth.revoked.subscribe();
    loop {
        tokio::select! {
            ended = revoked.recv() => match ended {
                Ok(id) if id == signed.id || id == "*" => break,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let _ = hub.sender.send((me, text.to_string()));
                }
                Some(Ok(_)) => {}
                _ => break,
            },
            outgoing = heard.recv() => match outgoing {
                Ok((from, text)) if from != me => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                // Too much at once, and some of it was missed: the whole
                // library, as after a reconnect.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let all = json!({ "event": "fs-change", "payload": { "paths": [crate::paths::LIBRARY] } });
                    if socket.send(Message::Text(all.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_what_nobody_wants_to_hear_about() {
        assert!(ignored(Path::new(".git/index")));
        assert!(ignored(Path::new("node_modules/a/b.md")));
        assert!(ignored(Path::new("notes/.tmpA1B2")));
        assert!(!ignored(Path::new("notes/a.md")));
    }

    #[test]
    fn only_this_site_may_listen() {
        let headers = |origin: &str| {
            let mut map = HeaderMap::new();
            map.insert(header::HOST, "127.0.0.1:8642".parse().unwrap());
            if !origin.is_empty() {
                map.insert(header::ORIGIN, origin.parse().unwrap());
            }
            map
        };
        assert!(same_origin(&headers("http://127.0.0.1:8642")));
        assert!(!same_origin(&headers("https://evil.example")));
        assert!(!same_origin(&headers("http://127.0.0.1:8642.evil.example")));
        assert!(!same_origin(&headers("")));
    }

    #[tokio::test]
    async fn a_change_on_disk_reaches_the_hub_in_virtual_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lib/notes")).unwrap();
        let roots =
            Arc::new(Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap());
        let hub = Hub::new();
        let mut heard = hub.subscribe();
        let _watcher = watch(roots.clone(), hub.clone()).unwrap();
        std::fs::write(dir.path().join("lib/notes/.tmp1"), "x").unwrap();
        std::fs::write(dir.path().join("lib/notes/a.md"), "x").unwrap();
        // The folder may be reported first; the file follows.
        let mut heard_all = String::new();
        let seen = tokio::time::timeout(Duration::from_secs(5), async {
            while let Ok((from, text)) = heard.recv().await {
                assert_eq!(from, 0);
                heard_all.push_str(&text);
                if text.contains("\"/library/notes/a.md\"") {
                    break;
                }
            }
        })
        .await;
        assert!(seen.is_ok(), "{heard_all}");
        assert!(!heard_all.contains(".tmp1"), "{heard_all}");
    }
}
