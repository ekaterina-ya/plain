// The HTTP surface, end to end against a temporary library.

use std::fs;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::paths::Roots;

struct Lib {
    dir: tempfile::TempDir,
    /// Signed in: every request carries the cookie unless it has its own.
    app: axum::Router,
    /// The same server without the cookie.
    bare: axum::Router,
    auth: Arc<crate::auth::Auth>,
    /// `plain_session=…` of the device `app` is.
    cookie: String,
}

fn signed(dir: tempfile::TempDir, roots: Roots, web: Option<std::path::PathBuf>) -> Lib {
    let (auth, token) = crate::auth::signed_for_tests(&dir.path().join("data"));
    let bare = crate::app(
        Arc::new(roots),
        crate::events::Hub::new(),
        auth.clone(),
        hosts(),
        web,
    );
    let cookie = format!("plain_session={token}");
    let app = crate::signed_in(bare.clone(), cookie.clone(), false);
    Lib {
        dir,
        app,
        bare,
        auth,
        cookie,
    }
}

const HOST: &str = "127.0.0.1:8642";

/// Behind Caddy, by its domain.
const PUBLIC: &str = "notes.example.com";

fn hosts() -> Vec<String> {
    vec![HOST.to_string(), PUBLIC.to_string()]
}

fn lib() -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    fs::create_dir_all(root.join("notes/assets")).unwrap();
    fs::write(root.join("notes/a.md"), "# A\n\nneedle here\n").unwrap();
    // «Привет» in windows-1251.
    fs::write(
        root.join("cp.md"),
        [0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2, b'\n'],
    )
    .unwrap();
    fs::write(root.join("notes/assets/p.png"), b"\x89PNG").unwrap();
    fs::write(dir.path().join("secret.md"), "s").unwrap();
    let roots = Roots::new(&root, &dir.path().join("data")).unwrap();
    signed(dir, roots, None)
}

async fn call(lib: &Lib, command: &str, args: Value) -> (StatusCode, Value) {
    let request = Request::post(format!("/api/invoke/{command}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-plain", "1")
        .header(header::HOST, HOST)
        .body(Body::from(args.to_string()))
        .unwrap();
    let response = lib.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(lib: &Lib, uri: &str) -> axum::response::Response {
    lib.app
        .clone()
        .oneshot(
            Request::get(uri)
                .header(header::HOST, HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn the_tree_comes_back_in_virtual_paths() {
    let lib = lib();
    let (status, tree) = call(
        &lib,
        "read_tree",
        json!({ "root": "/library", "extensions": ["md"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = tree.to_string();
    assert!(text.contains("\"/library/notes/a.md\""), "{text}");
    assert!(!text.contains(lib.dir.path().to_str().unwrap()));
}

#[tokio::test]
async fn a_file_is_read_with_its_encoding() {
    let lib = lib();
    let (status, info) = call(&lib, "read_file", json!({ "path": "/library/cp.md" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["path"], "/library/cp.md");
    assert_eq!(info["text"], "Привет\n");
    assert_eq!(info["encoding"], "windows-1251");
}

#[tokio::test]
async fn a_missing_file_says_so_without_the_real_path() {
    let lib = lib();
    let (status, error) = call(&lib, "read_file", json!({ "path": "/library/nope.md" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        !error.to_string().contains(lib.dir.path().to_str().unwrap()),
        "{error}"
    );
}

#[tokio::test]
async fn outside_the_library_is_refused() {
    let lib = lib();
    for path in ["/library/../secret.md", "/etc/passwd", "/library2/x"] {
        let (status, _) = call(&lib, "read_file", json!({ "path": path })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
}

#[tokio::test]
async fn search_finds_the_line() {
    let lib = lib();
    let request =
        json!({ "request": { "root": "/library", "query": "needle", "extensions": ["md"] } });
    let (status, answer) = call(&lib, "search_folder", request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["files"][0]["path"], "/library/notes/a.md");
    assert_eq!(answer["files"][0]["matches"][0]["line"], 3);
}

#[tokio::test]
async fn canonical_path_stays_virtual() {
    let lib = lib();
    let (_, path) = call(
        &lib,
        "canonical_path",
        json!({ "path": "/library/notes/a.md" }),
    )
    .await;
    assert_eq!(path, "/library/notes/a.md");
}

#[tokio::test]
async fn writing_goes_to_our_own_two_files_only() {
    let lib = lib();
    for path in [
        "/library/notes/a.md",
        "/data/drafts/d.json",
        "/data/trash/x.json",
        "/data/history/doc1/x.md",
    ] {
        let (status, _) = call(
            &lib,
            "write_text_atomic",
            json!({ "path": path, "text": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "# A\n\nneedle here\n");
    let (status, _) = call(
        &lib,
        "write_text_atomic",
        json!({ "path": "/data/state.json", "text": "{}" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(on_disk(&lib, "data/state.json"), "{}");
}

#[tokio::test]
async fn the_data_folder_is_listed_and_nothing_is_removed_from_it() {
    let lib = lib();
    fs::create_dir_all(lib.dir.path().join("data/history/doc1")).unwrap();
    fs::write(lib.dir.path().join("data/history/doc1/s.md"), "s").unwrap();
    let (_, list) = call(&lib, "fs_read_dir", json!({ "path": "/data/history/doc1" })).await;
    assert_eq!(
        list,
        json!([{ "name": "s.md", "isFile": true, "isDirectory": false }])
    );
    let (_, stat) = call(
        &lib,
        "fs_stat",
        json!({ "path": "/data/history/doc1/s.md" }),
    )
    .await;
    assert_eq!(stat["isFile"], true);
    let (_, none) = call(&lib, "fs_stat", json!({ "path": "/data/nope.json" })).await;
    assert_eq!(none, Value::Null);
    // Drafts, the only thing it removed, are in the browser now.
    let (status, _) = call(
        &lib,
        "fs_remove",
        json!({ "path": "/data/history/doc1/s.md" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_library_opens_itself_until_the_session_names_it() {
    let lib = lib();
    let (_, opening) = call(&lib, "take_opening", json!({})).await;
    assert_eq!(opening["folders"], json!(["/library"]));
    fs::write(
        lib.dir.path().join("data/state.json"),
        r#"{"library":"/library"}"#,
    )
    .unwrap();
    let (_, opening) = call(&lib, "take_opening", Value::Null).await;
    assert_eq!(opening["folders"], json!([]));
}

#[tokio::test]
async fn tags_and_backlinks_name_virtual_files() {
    let lib = lib();
    fs::write(
        lib.dir.path().join("lib/b.md"),
        "#idea see [a](notes/a.md)\n",
    )
    .unwrap();
    let (_, tags) = call(
        &lib,
        "collect_tags",
        json!({ "root": "/library", "extensions": ["md"] }),
    )
    .await;
    assert_eq!(tags[0]["files"], json!(["/library/b.md"]));
    let args = json!({ "root": "/library", "extensions": ["md"], "path": "/library/notes/a.md" });
    let (_, links) = call(&lib, "backlinks", args).await;
    assert_eq!(links[0]["path"], "/library/b.md");
}

#[tokio::test]
async fn the_data_folder_itself_stays() {
    let lib = lib();
    let (status, _) = call(
        &lib,
        "write_text_atomic",
        json!({ "path": "/data/", "text": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(lib.dir.path().join("data").is_dir());
}

#[tokio::test]
async fn a_large_document_fits() {
    let lib = lib();
    let text = "x".repeat(5 << 20);
    let request = write_request("/library/notes/a.md", &text, &hash("# A\n\nneedle here\n"));
    let (status, _) = call(&lib, "write_file_atomic", request).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn outside_the_roots_is_not_there() {
    let lib = lib();
    let (status, stat) = call(&lib, "fs_stat", json!({ "path": "/Users/x/a.md" })).await;
    assert_eq!((status, stat), (StatusCode::OK, Value::Null));
}

#[tokio::test]
async fn an_unknown_command_is_not_found() {
    let lib = lib();
    let (status, _) = call(&lib, "new_window", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn assets_come_from_the_library_only() {
    let lib = lib();
    let ok = get(&lib, "/api/asset?path=%2Flibrary%2Fnotes%2Fassets%2Fp.png").await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
    // Sandboxed: an .html or .svg from the library cannot script the app.
    assert!(ok.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .starts_with("sandbox"));
    let out = get(&lib, "/api/asset?path=%2Flibrary%2F..%2Fsecret.md").await;
    assert_eq!(out.status(), StatusCode::FORBIDDEN);
    let data = get(&lib, "/api/asset?path=%2Fdata%2Fstate.json").await;
    assert_eq!(data.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unknown_api_paths_are_not_the_page() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::create_dir_all(dir.path().join("web")).unwrap();
    fs::write(dir.path().join("web/index.html"), "<html>").unwrap();
    let roots = Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap();
    let web = dir.path().join("web");
    let lib = signed(dir, roots, Some(web));
    assert_eq!(
        get(&lib, "/api/nothing").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(get(&lib, "/some/route").await.status(), StatusCode::OK);
}

/* ------------------------------------------------------------- stage 2 */

fn on_disk(lib: &Lib, rel: &str) -> String {
    fs::read_to_string(lib.dir.path().join(rel)).unwrap()
}

fn hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

fn write_request(path: &str, text: &str, base: &str) -> Value {
    json!({ "request": {
        "path": path, "text": text, "encoding": "utf-8", "bom": false, "eol": "lf",
        "baseHash": base, "snapshotId": "doc1",
    } })
}

#[tokio::test]
async fn a_document_is_written_when_nobody_else_has_and_kept_in_the_history() {
    let lib = lib();
    let base = hash("# A\n\nneedle here\n");
    let (status, written) = call(
        &lib,
        "write_file_atomic",
        write_request("/library/notes/a.md", "new\n", &base),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    assert_eq!(written["hash"], hash("new\n"));
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "new\n");

    let (_, snapshots) = call(&lib, "list_snapshots", json!({ "id": "doc1" })).await;
    let path = snapshots[0]["path"].as_str().unwrap().to_string();
    assert!(path.starts_with("/data/history/doc1/"), "{path}");
    let (_, copy) = call(&lib, "read_file", json!({ "path": path })).await;
    assert_eq!(copy["text"], "new\n");
    let (status, _) = call(&lib, "delete_snapshot", json!({ "path": path })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, snapshots) = call(&lib, "list_snapshots", json!({ "id": "doc1" })).await;
    assert_eq!(snapshots, json!([]));
}

#[tokio::test]
async fn a_stale_hash_is_a_conflict() {
    let lib = lib();
    let (status, error) = call(
        &lib,
        "write_file_atomic",
        write_request("/library/notes/a.md", "mine\n", &hash("old")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["kind"], "conflict");
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "# A\n\nneedle here\n");
}

#[tokio::test]
async fn documents_are_written_in_the_library_only() {
    let lib = lib();
    for path in ["/data/state.json", "/library/../secret.md", "/etc/x.md"] {
        let (status, _) = call(
            &lib,
            "write_file_atomic",
            write_request(path, "x", &hash("s")),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
    assert_eq!(on_disk(&lib, "secret.md"), "s");
    // Only snapshots go, and only from the history.
    let (status, _) = call(
        &lib,
        "delete_snapshot",
        json!({ "path": "/data/state.json" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn files_and_folders_are_made_and_renamed_in_the_library() {
    let lib = lib();
    let (status, _) = call(&lib, "create_file", json!({ "path": "/library/new.md" })).await;
    assert_eq!(status, StatusCode::OK);
    let (_, error) = call(&lib, "create_file", json!({ "path": "/library/new.md" })).await;
    assert_eq!(error["kind"], "exists");
    let (status, _) = call(&lib, "create_dir", json!({ "path": "/library/folder" })).await;
    assert_eq!(status, StatusCode::OK);
    let args = json!({ "from": "/library/new.md", "to": "/library/folder/moved.md" });
    let (status, _) = call(&lib, "rename_path", args).await;
    assert_eq!(status, StatusCode::OK);
    assert!(lib.dir.path().join("lib/folder/moved.md").is_file());
    for args in [
        json!({ "from": "/library", "to": "/library/x" }),
        json!({ "from": "/library/folder", "to": "/library/" }),
        json!({ "from": "/library/folder", "to": "/data/x" }),
    ] {
        let (status, _) = call(&lib, "rename_path", args.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{args}");
    }
}

#[tokio::test]
async fn the_trash_keeps_what_was_deleted_and_gives_it_back() {
    let lib = lib();
    let (status, _) = call(&lib, "trash_path", json!({ "path": "/library/notes/a.md" })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!lib.dir.path().join("lib/notes/a.md").exists());
    let (_, list) = call(&lib, "trash_list", json!({})).await;
    assert_eq!(list[0]["path"], "/library/notes/a.md");
    assert_eq!(list[0]["dir"], false);
    let id = list[0]["id"].as_str().unwrap().to_string();

    // Its place taken in the meantime: nothing is overwritten.
    fs::write(lib.dir.path().join("lib/notes/a.md"), "other").unwrap();
    let (status, error) = call(&lib, "trash_restore", json!({ "id": id })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error, "/library/notes/a.md already exists");
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "other");

    fs::remove_file(lib.dir.path().join("lib/notes/a.md")).unwrap();
    let (status, path) = call(&lib, "trash_restore", json!({ "id": id })).await;
    assert_eq!(
        (status, path),
        (StatusCode::OK, json!("/library/notes/a.md"))
    );
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "# A\n\nneedle here\n");
    let (_, list) = call(&lib, "trash_list", json!({})).await;
    assert_eq!(list, json!([]));
}

#[tokio::test]
async fn a_folder_goes_to_the_trash_whole_and_the_library_never_does() {
    let lib = lib();
    let (status, _) = call(&lib, "trash_path", json!({ "path": "/library/notes" })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&lib, "trash_path", json!({ "path": "/library/notes" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for path in [
        "/library",
        "/library/",
        "/data/state.json",
        "/library/../secret.md",
    ] {
        let (status, _) = call(&lib, "trash_path", json!({ "path": path })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}");
    }
    let (_, list) = call(&lib, "trash_list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["dir"], true);
    let id = list[0]["id"].as_str().unwrap().to_string();
    for id in ["../state", "", "x/y"] {
        let (status, _) = call(&lib, "trash_restore", json!({ "id": id })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{id}");
    }
    call(&lib, "trash_restore", json!({ "id": id })).await;
    assert!(lib.dir.path().join("lib/notes/assets/p.png").is_file());
}

#[test]
fn the_trash_forgets_after_thirty_days() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(dir.path().join("lib/a.md"), "a").unwrap();
    let roots = Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap();
    let real = roots.library().join("a.md");
    crate::trashcan::put(&roots, real.to_str().unwrap()).unwrap();
    let now = std::time::SystemTime::now();
    crate::trashcan::sweep(&roots, now + std::time::Duration::from_secs(29 * 86_400));
    assert_eq!(crate::trashcan::list(&roots).len(), 1);
    crate::trashcan::sweep(&roots, now + std::time::Duration::from_secs(31 * 86_400));
    assert_eq!(crate::trashcan::list(&roots).len(), 0);
    assert_eq!(
        fs::read_dir(dir.path().join("data/trash")).unwrap().count(),
        0
    );
}

async fn upload(lib: &Lib, path: &str, bytes: &'static [u8]) -> StatusCode {
    let request = Request::post(format!("/api/upload?path={path}"))
        .header("x-plain", "1")
        .header(header::HOST, HOST)
        .body(Body::from(bytes))
        .unwrap();
    lib.app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn a_picture_is_uploaded_next_to_the_document_and_never_over_a_file() {
    let lib = lib();
    let (status, _) = call(&lib, "create_dir", json!({ "path": "/library/assets" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        upload(&lib, "%2Flibrary%2Fassets%2Fx.png", b"png").await,
        StatusCode::OK
    );
    assert_eq!(
        fs::read(lib.dir.path().join("lib/assets/x.png")).unwrap(),
        b"png"
    );
    assert_eq!(
        upload(&lib, "%2Flibrary%2Fassets%2Fx.png", b"again").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        fs::read(lib.dir.path().join("lib/assets/x.png")).unwrap(),
        b"png"
    );
    assert_eq!(
        upload(&lib, "%2Flibrary%2F..%2Fx.png", b"x").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        upload(&lib, "%2Fdata%2Fx.png", b"x").await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_write_without_the_header_is_refused() {
    let lib = lib();
    let request = Request::post("/api/invoke/trash_path")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::HOST, HOST)
        .body(Body::from(r#"{"path":"/library/notes/a.md"}"#))
        .unwrap();
    let response = lib.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(lib.dir.path().join("lib/notes/a.md").exists());
    let request = Request::post("/api/upload?path=%2Flibrary%2Fx.png")
        .header(header::HOST, HOST)
        .body(Body::from("x"))
        .unwrap();
    let response = lib.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn another_name_for_this_machine_is_refused() {
    let lib = lib();
    let request = Request::get("/api/asset?path=%2Flibrary%2Fnotes%2Fassets%2Fp.png")
        .header(header::HOST, "attacker.example:8642")
        .body(Body::empty())
        .unwrap();
    let response = lib.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_lost_answer_sent_again_is_not_a_conflict() {
    let lib = lib();
    let request = write_request(
        "/library/notes/a.md",
        "new\n",
        &hash("# A\n\nneedle here\n"),
    );
    call(&lib, "write_file_atomic", request.clone()).await;
    let (status, written) = call(&lib, "write_file_atomic", request).await;
    assert_eq!(status, StatusCode::OK, "{written}");
    assert_eq!(written["hash"], hash("new\n"));
}

#[tokio::test]
async fn nothing_is_overwritten_unseen_or_written_where_the_tree_does_not_look() {
    let lib = lib();
    let mut request = write_request("/library/notes/a.md", "x", "");
    request["request"]["baseHash"] = Value::Null;
    let (_, error) = call(&lib, "write_file_atomic", request).await;
    assert_eq!(error["kind"], "conflict");
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "# A\n\nneedle here\n");
    fs::create_dir_all(lib.dir.path().join("lib/.git/hooks")).unwrap();
    for (command, args) in [
        (
            "create_file",
            json!({ "path": "/library/.git/hooks/pre-commit" }),
        ),
        ("create_dir", json!({ "path": "/library/.vscode" })),
        (
            "rename_path",
            json!({ "from": "/library/notes/a.md", "to": "/library/.git/hooks/a" }),
        ),
        ("trash_path", json!({ "path": "/library/.git" })),
    ] {
        let (status, _) = call(&lib, command, args).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{command}");
    }
    assert_eq!(
        upload(&lib, "%2Flibrary%2F.git%2Fhooks%2Fpost", b"x").await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_forged_note_in_the_trash_puts_nothing_in_a_dot_folder() {
    let lib = lib();
    let trash = lib.dir.path().join("data/trash");
    fs::create_dir_all(&trash).unwrap();
    fs::write(trash.join("x"), "evil").unwrap();
    fs::write(
        trash.join("x.json"),
        r#"{"path":"/library/.git/hooks/pre-commit","deletedAt":0,"dir":false}"#,
    )
    .unwrap();
    let (status, _) = call(&lib, "trash_restore", json!({ "id": "x" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!lib.dir.path().join("lib/.git").exists());
}

#[tokio::test]
async fn a_file_goes_back_even_when_its_folder_has_gone() {
    let lib = lib();
    call(&lib, "trash_path", json!({ "path": "/library/notes/a.md" })).await;
    fs::remove_dir_all(lib.dir.path().join("lib/notes")).unwrap();
    let (_, list) = call(&lib, "trash_list", json!({})).await;
    let id = list[0]["id"].clone();
    let (status, _) = call(&lib, "trash_restore", json!({ "id": id })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(on_disk(&lib, "lib/notes/a.md"), "# A\n\nneedle here\n");
}

#[tokio::test]
async fn the_socket_answers_to_this_server_s_names_only() {
    let lib = lib();
    let request = Request::get("/api/events")
        .header(header::HOST, "attacker.example:8642")
        .header(header::ORIGIN, "http://attacker.example:8642")
        .body(Body::empty())
        .unwrap();
    let response = lib.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/* ------------------------------------------------------------ signing in */

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.unwrap()
}

fn location(response: &axum::response::Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

fn set_cookie(response: &axum::response::Response) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_string()
}

fn sign_in_request(host: &str, password: &str, code: &str) -> Request<Body> {
    Request::post("/login")
        .header(header::HOST, host)
        .header(header::ORIGIN, format!("http://{host}"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-forwarded-for", "203.0.113.9, 198.51.100.7")
        .body(Body::from(format!("password={password}&code={code}")))
        .unwrap()
}

#[tokio::test]
async fn without_a_session_there_is_only_the_sign_in_page() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::create_dir_all(dir.path().join("web/assets")).unwrap();
    fs::write(dir.path().join("web/index.html"), "<html>").unwrap();
    fs::write(dir.path().join("web/assets/app.js"), "code").unwrap();
    let roots = Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap();
    let web = dir.path().join("web");
    let lib = signed(dir, roots, Some(web));
    let get = |uri: &str| {
        Request::get(uri)
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap()
    };
    for page in ["/", "/assets/app.js", "/devices"] {
        let response = send(&lib.bare, get(page)).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{page}");
        assert_eq!(location(&response), "/login");
    }
    for api in ["/api/events", "/api/asset?path=%2Flibrary%2Fa.md"] {
        assert_eq!(
            send(&lib.bare, get(api)).await.status(),
            StatusCode::UNAUTHORIZED,
            "{api}"
        );
    }
    let (status, _) = {
        let request = Request::post("/api/invoke/read_tree")
            .header(header::HOST, HOST)
            .header("x-plain", "1")
            .body(Body::from("{}"))
            .unwrap();
        (send(&lib.bare, request).await.status(), ())
    };
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let page = send(&lib.bare, get("/login")).await;
    assert_eq!(page.status(), StatusCode::OK);
    // Signed in already: the app.
    let again = send(&lib.app, get("/login")).await;
    assert_eq!(location(&again), "/");
}

#[tokio::test]
async fn signing_in_sets_the_cookie_and_names_the_device() {
    let lib = lib();
    let code = crate::auth::code_for_tests();
    let response = send(&lib.bare, sign_in_request(HOST, "a+test+password", &code)).await;
    assert_eq!(location(&response), "/");
    let cookie = set_cookie(&response);
    assert!(cookie.starts_with("plain_session="));
    for flag in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=2592000"] {
        assert!(cookie.contains(flag), "{cookie}");
    }
    // Over http on this machine Safari would drop a Secure cookie.
    assert!(!cookie.contains("Secure"));
    let ip = lib
        .auth
        .sessions()
        .iter()
        .find(|s| s.id != "open")
        .unwrap()
        .ip
        .clone();
    assert_eq!(ip, "198.51.100.7");
    // The cookie works.
    let token = cookie.split(';').next().unwrap().to_string();
    let request = Request::get("/api/asset?path=%2Flibrary%2Fcp.md")
        .header(header::HOST, HOST)
        .header(header::COOKIE, token)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&lib.bare, request).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn behind_the_domain_the_cookie_is_secure() {
    let lib = lib();
    let code = crate::auth::code_for_tests();
    let response = send(&lib.bare, sign_in_request(PUBLIC, "a+test+password", &code)).await;
    assert!(set_cookie(&response).ends_with("; Secure"));
}

#[tokio::test]
async fn a_wrong_try_and_a_foreign_form_get_nothing() {
    let lib = lib();
    let response = send(&lib.bare, sign_in_request(HOST, "wrong", "123456")).await;
    assert_eq!(location(&response), "/login?e=1");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    let mut foreign = sign_in_request(HOST, "a+test+password", &crate::auth::code_for_tests());
    foreign
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(
        send(&lib.bare, foreign).await.status(),
        StatusCode::FORBIDDEN
    );
}

fn logout_request(uri: &str, origin: Option<&str>) -> Request<Body> {
    let mut request = Request::post(uri).header(header::HOST, HOST);
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    request.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn signing_out_this_device_another_or_all() {
    let lib = lib();
    let code = crate::auth::code_for_tests();
    send(&lib.bare, sign_in_request(HOST, "a+test+password", &code)).await;
    let other = lib
        .auth
        .sessions()
        .into_iter()
        .find(|s| s.id != "open")
        .unwrap()
        .id;
    // Only from this site's own page.
    let forged = send(
        &lib.app,
        logout_request(&format!("/logout?id={other}"), None),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);
    let origin = Some("http://127.0.0.1:8642");
    let response = send(
        &lib.app,
        logout_request(&format!("/logout?id={other}"), origin),
    )
    .await;
    assert_eq!(location(&response), "/devices");
    assert_eq!(lib.auth.sessions().len(), 1);
    // This device: back to signing in, the cookie gone.
    let response = send(&lib.app, logout_request("/logout", origin)).await;
    assert_eq!(location(&response), "/login");
    assert!(set_cookie(&response).contains("Max-Age=0"));
    assert!(lib.auth.sessions().is_empty());
    let (status, _) = call(
        &lib,
        "read_tree",
        json!({ "root": "/library", "extensions": ["md"] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn everywhere_means_everyone() {
    let lib = lib();
    send(
        &lib.bare,
        sign_in_request(HOST, "a+test+password", &crate::auth::code_for_tests()),
    )
    .await;
    assert_eq!(lib.auth.sessions().len(), 2);
    let response = send(
        &lib.app,
        logout_request("/logout-all", Some("http://127.0.0.1:8642")),
    )
    .await;
    assert_eq!(location(&response), "/login");
    assert!(lib.auth.sessions().is_empty());
}

#[tokio::test]
async fn the_devices_page_lists_them() {
    let lib = lib();
    let request = Request::get("/devices")
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    let response = send(&lib.app, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(page.contains("this device"));
    assert!(page.contains("sign out everywhere"));
}

#[tokio::test]
async fn a_device_signed_out_stops_hearing_at_once() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let lib = lib();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // The server knows itself by this port's name.
    let dir = lib.dir.path().to_path_buf();
    let roots = Roots::new(&dir.join("lib"), &dir.join("data")).unwrap();
    let host = format!("127.0.0.1:{port}");
    let app = crate::app(
        Arc::new(roots),
        crate::events::Hub::new(),
        lib.auth.clone(),
        vec![host.clone()],
        None,
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut request = format!("ws://{host}/api/events")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert(header::COOKIE, lib.cookie.parse().unwrap());
    request
        .headers_mut()
        .insert(header::ORIGIN, format!("http://{host}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    lib.auth.sign_out(Some("open"));
    let closed = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await;
    assert!(matches!(
        closed,
        Ok(None)
            | Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))))
            | Ok(Some(Err(_)))
    ));
}

/* ------------------------------------------------------------- deploying */

#[tokio::test]
async fn search_engines_are_told_to_stay_out() {
    let lib = lib();
    let robots = send(
        &lib.bare,
        Request::get("/robots.txt")
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(robots.status(), StatusCode::OK);
    assert_eq!(robots.headers()["x-robots-tag"], "noindex, nofollow");
    let text = to_bytes(robots.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&text[..], b"User-agent: *\nDisallow: /\n");
    let login = send(
        &lib.bare,
        Request::get("/login")
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(login.headers()["x-robots-tag"], "noindex, nofollow");
    let page = to_bytes(login.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&page)
        .contains("<meta name=\"robots\" content=\"noindex, nofollow\">"));
    // Even a refusal says so.
    let refused = send(
        &lib.bare,
        Request::get("/")
            .header(header::HOST, "elsewhere.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.headers()["x-robots-tag"], "noindex, nofollow");
}

#[test]
fn it_answers_to_the_address_it_listens_on() {
    let args = |list: &[&str]| {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    };
    let bridge = crate::options(args(&[
        "--library",
        "/srv/notes",
        "--data",
        "/var/lib/plain-web",
        "--listen",
        "172.18.0.1:8642",
        "--host",
        "notes.example.com",
    ]))
    .unwrap();
    assert_eq!(bridge.listen.to_string(), "172.18.0.1:8642");
    assert!(bridge.hosts.contains(&"notes.example.com".to_string()));
    assert!(bridge.hosts.contains(&"172.18.0.1:8642".to_string()));
    assert!(!bridge.hosts.iter().any(|h| h.starts_with("localhost:8642")));
    let local = crate::options(args(&["--library", "a", "--data", "b"])).unwrap();
    assert!(local.hosts.contains(&"127.0.0.1:8642".to_string()));
    assert!(local.hosts.contains(&"localhost:8642".to_string()));
    assert!(crate::options(args(&["--library", "a", "--data", "b", "--port", "1"])).is_err());
    assert!(!local.open);
    let open = crate::options(args(&["--dev-open", "--library", "a", "--data", "b"])).unwrap();
    assert!(open.open);
    assert!(crate::options(args(&[
        "--library",
        "a",
        "--data",
        "b",
        "--listen",
        "172.18.0.1:8642",
        "--dev-open",
    ]))
    .is_err());
}

#[tokio::test]
async fn a_taken_name_is_said_so_on_upload() {
    let lib = lib();
    let request = Request::post("/api/upload?path=%2Flibrary%2Fcp.md")
        .header("x-plain", "1")
        .header(header::HOST, HOST)
        .body(Body::from("x"))
        .unwrap();
    let response = send(&lib.app, request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["kind"], "exists");
    assert!(!body.to_string().contains(lib.dir.path().to_str().unwrap()));
}

#[tokio::test]
async fn the_server_s_secrets_are_not_read_back_out() {
    let lib = lib();
    fs::write(lib.dir.path().join("data/sessions.json"), "[]").unwrap();
    for name in ["auth.json", "sessions.json"] {
        for command in ["read_file", "hash_file"] {
            let (status, answer) =
                call(&lib, command, json!({ "path": format!("/data/{name}") })).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{command} {name}");
            assert!(!answer.to_string().contains("argon2"), "{answer}");
        }
    }
    // The page's own files stay readable.
    fs::write(lib.dir.path().join("data/settings.json"), "{}").unwrap();
    assert_eq!(
        call(&lib, "read_file", json!({ "path": "/data/settings.json" }))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_home_screen_icon_and_manifest_need_no_session() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::create_dir_all(dir.path().join("web")).unwrap();
    fs::write(dir.path().join("web/index.html"), "<html>").unwrap();
    for name in [
        "manifest.webmanifest",
        "apple-touch-icon.png",
        "icon-192.png",
        "icon-512.png",
    ] {
        fs::write(dir.path().join("web").join(name), "x").unwrap();
    }
    let roots = Roots::new(&dir.path().join("lib"), &dir.path().join("data")).unwrap();
    let web = dir.path().join("web");
    let lib = signed(dir, roots, Some(web));
    for path in [
        "/manifest.webmanifest",
        "/apple-touch-icon.png",
        "/icon-192.png",
        "/icon-512.png",
    ] {
        let request = Request::get(path)
            .header(header::HOST, HOST)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            send(&lib.bare, request).await.status(),
            StatusCode::OK,
            "{path}"
        );
    }
    let page = Request::get("/index.html")
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&lib.bare, page).await.status(), StatusCode::SEE_OTHER);
}
