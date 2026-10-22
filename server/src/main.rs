// Plain Web: the library folder on this machine, opened from a browser
// (ТЗ-Plain-Web §2). Listens on 127.0.0.1, or on the Docker bridge for the
// Caddy in front of it (`--listen`); signing in is auth.rs.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

// The desktop's own code, unchanged; `tauri` here is server/tauri-shim.
#[allow(dead_code)]
#[path = "../../src-tauri/src/fs.rs"]
mod fs;
#[allow(dead_code)]
#[path = "../../src-tauri/src/history.rs"]
mod history;
#[allow(dead_code)]
#[path = "../../src-tauri/src/search.rs"]
mod search;
#[allow(dead_code)]
#[path = "../../src-tauri/src/tree.rs"]
mod tree;

mod api;
mod auth;
mod events;
mod paths;
mod trashcan;

/// What history.rs asks the desktop for (lib.rs `data_dir`).
pub fn data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    app.data_dir().to_path_buf()
}

/// The desktop's policy (tauri.conf.json) without the Tauri schemes.
const CSP: &str = "default-src 'self'; img-src 'self' https: data: blob:; \
    style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self'; \
    object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

/// Every request that changes something carries `X-Plain`. A page from
/// another site can post to 127.0.0.1 without asking, but not with a header
/// of its own: that takes a CORS answer this server never gives.
async fn ours(request: Request, next: Next) -> Response {
    if request.headers().contains_key("x-plain") {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Only the names this server goes by. Another name that resolves to
/// 127.0.0.1 makes its page same-origin with this one (DNS rebinding), and
/// `X-Plain` and the socket's origin check would let it through.
async fn named(State(hosts): State<Arc<Vec<String>>>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok());
    if host.is_some_and(|host| hosts.iter().any(|h| h == host)) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Requests carry this cookie: every one under `--dev-open` (a stale one
/// from before is in the way there), those without their own in the tests.
#[cfg(debug_assertions)]
pub fn signed_in(app: Router, cookie: String, always: bool) -> Router {
    app.layer(middleware::map_request(move |mut request: Request| {
        if always || !request.headers().contains_key(header::COOKIE) {
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        async move { request }
    }))
}

pub fn app(
    roots: Arc<paths::Roots>,
    hub: Arc<events::Hub>,
    auth: Arc<auth::Auth>,
    hosts: Vec<String>,
    static_dir: Option<PathBuf>,
) -> Router {
    // A whole document, or a picture; axum's default is 2 MB.
    let limit = DefaultBodyLimit::max(64 << 20);
    let mut router = Router::new()
        .route("/api/invoke/{command}", post(api::invoke))
        .route("/api/upload", post(api::upload))
        .route_layer(middleware::from_fn(ours))
        .layer(limit)
        .route("/api/asset", get(api::asset))
        .route("/api/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .with_state(roots)
        .merge(
            Router::new()
                .route("/api/events", get(events::socket))
                .with_state(hub),
        )
        .merge(
            Router::new()
                .route("/login", get(auth::login_page).post(auth::login))
                .route(
                    "/robots.txt",
                    get(|| async { "User-agent: *\nDisallow: /\n" }),
                )
                .route("/devices", get(auth::devices))
                .route("/logout", post(auth::logout))
                .route("/logout-all", post(auth::logout_all))
                .with_state(auth.clone()),
        );
    if let Some(dir) = static_dir {
        let index = ServeFile::new(dir.join("index.html"));
        router = router.fallback_service(ServeDir::new(dir).fallback(index));
    }
    router
        .layer(middleware::from_fn_with_state(auth, auth::guard))
        .layer(middleware::from_fn_with_state(Arc::new(hosts), named))
        // Outside `named` and `guard`, so a refusal and the way to sign in carry it too (§7).
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex, nofollow"),
        ))
        // `if_not_present`: an asset brings its own, stricter one.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
}

struct Options {
    library: PathBuf,
    data: PathBuf,
    listen: SocketAddr,
    hosts: Vec<String>,
    static_dir: Option<PathBuf>,
    // `--dev-open`: never set in a release build.
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    open: bool,
}

fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let usage = "usage: plain-web --library <dir> --data <dir> [--listen 127.0.0.1:8642] \
        [--host <name>]… [--static <dir>] [--dev-open]\n       plain-web init|reset-password --data <dir>";
    let (mut library, mut data, mut static_dir) = (None, None, None);
    let mut listen = SocketAddr::from(([127, 0, 0, 1], 8642));
    let mut hosts = Vec::new();
    let mut open = false;
    while let Some(flag) = args.next() {
        // Signed in without a password, to look at the app on this machine:
        // a debug build, on loopback only.
        if flag == "--dev-open" && cfg!(debug_assertions) {
            open = true;
            continue;
        }
        let value = args.next().ok_or(usage)?;
        match flag.as_str() {
            "--library" => library = Some(PathBuf::from(value)),
            "--data" => data = Some(PathBuf::from(value)),
            "--listen" => listen = value.parse().map_err(|_| usage)?,
            "--host" => hosts.push(value),
            "--static" => static_dir = Some(PathBuf::from(value)),
            _ => return Err(usage.into()),
        }
    }
    if open && !listen.ip().is_loopback() {
        return Err("--dev-open: on 127.0.0.1 only".into());
    }
    // Its own address is a name it answers to: the health checks on the
    // server ask it that way.
    hosts.push(listen.to_string());
    if listen.ip().is_loopback() {
        hosts.push(format!("localhost:{}", listen.port()));
    }
    // `npm run dev:web`: Vite's proxy passes its own name on.
    if cfg!(debug_assertions) {
        hosts.push("localhost:1420".into());
    }
    Ok(Options {
        library: library.ok_or(usage)?,
        data: data.ok_or(usage)?,
        listen,
        hosts,
        static_dir,
        open,
    })
}

/// `plain-web init|reset-password --data <dir>`, before anything is served.
fn command() -> Option<Result<(), String>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, data) = match args.as_slice() {
        [command, flag, data] if flag == "--data" => (command.as_str(), PathBuf::from(data)),
        [command, ..] if command == "init" || command == "reset-password" => {
            return Some(Err(format!("usage: plain-web {command} --data <dir>")))
        }
        _ => return None,
    };
    match command {
        "init" => Some(auth::init(&data)),
        "reset-password" => Some(auth::reset_password(&data)),
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    if let Some(done) = command() {
        if let Err(error) = done {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let options = options(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let roots = paths::Roots::new(&options.library, &options.data).unwrap_or_else(|error| {
        eprintln!("library or data folder: {error}");
        std::process::exit(1);
    });
    let roots = Arc::new(roots);
    let _lock = auth::lock(roots.data()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    let auth = auth::Auth::load(roots.data()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    let hub = events::Hub::new();
    // Without it changes from elsewhere show up on reconnect and on focus
    // only (app/watcher.ts) — worse, but no reason not to serve.
    let _watcher = events::watch(roots.clone(), hub.clone())
        .map_err(|error| eprintln!("not watching the library: {error}"))
        .ok();
    // Thirty days of history and of trash, swept now and once a day.
    let sweeping = roots.clone();
    tokio::spawn(async move {
        let mut day = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            day.tick().await;
            let roots = sweeping.clone();
            let _ = tokio::task::spawn_blocking(move || {
                history::clean(&tauri::AppHandle::<tauri::Wry>::new(roots.data()));
                trashcan::sweep(&roots, std::time::SystemTime::now());
            })
            .await;
        }
    });
    let address = options.listen;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{address}: {error}");
            std::process::exit(1);
        });
    eprintln!("plain-web on http://{address}");
    #[cfg(debug_assertions)]
    let open = options.open.then(|| auth.open_session());
    let app = app(
        roots,
        hub,
        Arc::new(auth),
        options.hosts,
        options.static_dir,
    );
    #[cfg(debug_assertions)]
    let app = match open {
        Some(token) => {
            eprintln!("--dev-open: no sign-in");
            signed_in(app, format!("plain_session={token}"), true)
        }
        None => app,
    };
    axum::serve(listener, app).await.expect("server stopped");
}

#[cfg(test)]
mod tests;
