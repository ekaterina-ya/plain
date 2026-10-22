// Signing in (ТЗ-Plain-Web §3.2–3.3): one owner, a password and a code from
// an authenticator app, a cookie per device for thirty days. Without one the
// server answers nothing but the sign-in page.
//
// The secrets are in `<data>/auth.json`, the sessions in `<data>/sessions.json`
// (0600, both): the provider's backup takes them with the rest of the state
// (§6), and the token itself is only ever in the cookie — the file has its
// hash. `plain-web init` and `reset-password` are the only ways to set them.

use std::collections::HashMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Form};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use totp_rs::{Algorithm, Secret, TOTP};

const DAY: u64 = 24 * 60 * 60;
/// A device stays signed in this long after it was last used (§3.2).
const LIFE: u64 = 30 * DAY;
/// `seen` is written down at most this often, not on every request.
const RENEW: u64 = 60 * 60;
/// Five wrong tries from one place, and that place waits (§3.2).
const TRIES: u32 = 5;
/// The right password with a wrong code this many times, from anywhere, and
/// everyone waits: only someone who has the password can cause it, and the
/// codes are what is left to guess.
const NEAR_TRIES: u32 = 10;
const PAUSE: u64 = 15 * 60;
const COOKIE: &str = "plain_session";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Secrets {
    /// argon2id, PHC string.
    password: String,
    /// base32, as the authenticator app was given it.
    totp: String,
    /// The last code's time step: the same code is never taken twice.
    last_step: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    hash: String,
    pub created: u64,
    pub seen: u64,
    pub agent: String,
    pub ip: String,
}

#[derive(Default)]
struct Limits {
    /// Wrong tries and when the count runs out, by address (IPv6 by /64).
    by_place: HashMap<String, (u32, u64)>,
    near: (u32, u64),
}

pub enum Refusal {
    Wrong,
    Paused,
}

pub struct Auth {
    dir: PathBuf,
    secrets: Mutex<Secrets>,
    sessions: Mutex<Vec<Session>>,
    limits: Mutex<Limits>,
    /// A session id that has just ended, or `*` for all: its sockets close
    /// (events.rs).
    pub revoked: broadcast::Sender<String>,
    /// Sign-ins queue here, not on a blocking thread each: a flood of them
    /// must not take the threads every file operation needs.
    gate: tokio::sync::Mutex<()>,
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0; bytes];
    OsRng.fill_bytes(&mut buffer);
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// Temp-and-rename, as every other file of ours, and the owner's only —
/// even over a copy that came back from a backup with other rights.
fn save(dir: &Path, name: &str, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let target = dir.join(name);
    let temp = crate::fs::stage(dir, &bytes).map_err(|e| e.message().to_string())?;
    crate::fs::commit(&target, &temp, target.exists()).map_err(|e| e.message().to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One server per data folder, and no `init` or `reset-password` under a
/// running one: it holds the secrets and sessions in memory and would write
/// the old ones back. The lock goes with the process.
pub fn lock(dir: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(dir.join("plain-web.lock")).map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| format!("plain-web is running on {}: stop it first", dir.display()))?;
    Ok(file)
}

fn totp(secret: &str) -> Option<TOTP> {
    let bytes = Secret::Encoded(secret.to_string()).to_bytes().ok()?;
    TOTP::new(Algorithm::SHA1, 6, 1, 30, bytes).ok()
}

/// The step of the code, if it is one of the last, this or the next 30 s —
/// what Google Authenticator's clock may be off by.
fn code_step(secret: &str, code: &str, now: u64) -> Option<u64> {
    let totp = totp(secret)?;
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let step = now / 30;
    [step.saturating_sub(1), step, step + 1]
        .into_iter()
        .find(|s| same(totp.generate(s * 30).as_bytes(), code.as_bytes()))
}

/// Without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

/// One address, or one IPv6 /64: a whole /64 is one machine's to hand out.
fn place(ip: &str) -> String {
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_some() => {
            v6.to_ipv4_mapped().unwrap().to_string()
        }
        Ok(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => ip.to_string(),
    }
}

/// Another wrong try; the count lasts fifteen minutes from the latest.
fn bump(entry: &mut (u32, u64), now: u64) {
    if entry.1 <= now {
        entry.0 = 0;
    }
    entry.0 += 1;
    entry.1 = now + PAUSE;
}

impl Auth {
    /// What `init` writes: a new password and a new secret, nobody signed
    /// in; `last_step` is the code init was shown, spent.
    pub fn create(dir: &Path, password: &str, secret: &str, last_step: u64) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let secrets = Secrets {
            password: hash_password(password)?,
            totp: secret.to_string(),
            last_step,
        };
        save(dir, "auth.json", &secrets)?;
        save(dir, "sessions.json", &Vec::<Session>::new())
    }

    /// `reset-password`: the authenticator stays, every device signs in again.
    pub fn reset_password(dir: &Path, password: &str) -> Result<(), String> {
        let mut secrets = read_secrets(dir)?;
        secrets.password = hash_password(password)?;
        save(dir, "auth.json", &secrets)?;
        save(dir, "sessions.json", &Vec::<Session>::new())
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let secrets = read_secrets(dir)?;
        let sessions = match std::fs::read_to_string(dir.join("sessions.json")) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                eprintln!("sessions.json: {error}; every device signs in again");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            secrets: Mutex::new(secrets),
            sessions: Mutex::new(sessions),
            limits: Mutex::default(),
            revoked: broadcast::channel(16).0,
            gate: tokio::sync::Mutex::new(()),
        })
    }

    /// The password, then the code; a new session's token, or why not. One
    /// at a time: the code's step is checked and kept under one lock, and
    /// argon2's 19 MiB is not spent in parallel for whoever asks.
    pub fn sign_in(
        &self,
        password: &str,
        code: &str,
        ip: &str,
        agent: &str,
        now: u64,
    ) -> Result<String, Refusal> {
        let key = place(ip);
        // The count is read and raised under the same lock as the check:
        // tries sent all at once wait their turn and are counted, not let
        // through on a count none of them has raised yet.
        let mut secrets = self.secrets.lock().unwrap();
        {
            let mut limits = self.limits.lock().unwrap();
            limits.by_place.retain(|_, entry| entry.1 > now);
            let paused = |entry: &(u32, u64)| entry.0 >= TRIES && entry.1 > now;
            if limits.by_place.get(&key).is_some_and(paused)
                || (limits.near.0 >= NEAR_TRIES && limits.near.1 > now)
            {
                return Err(Refusal::Paused);
            }
        }
        let right = PasswordHash::new(&secrets.password).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        });
        let step = right
            .then(|| code_step(&secrets.totp, code, now))
            .flatten()
            .filter(|step| *step > secrets.last_step);
        let Some(step) = step else {
            let mut limits = self.limits.lock().unwrap();
            bump(limits.by_place.entry(key).or_default(), now);
            if right {
                bump(&mut limits.near, now);
            }
            return Err(Refusal::Wrong);
        };
        secrets.last_step = step;
        if let Err(error) = save(&self.dir, "auth.json", &*secrets) {
            eprintln!("auth.json: {error}");
        }
        drop(secrets);
        self.limits.lock().unwrap().by_place.remove(&key);
        let token = random_hex(32);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.push(Session {
            id: random_hex(6),
            hash: token_hash(&token),
            created: now,
            seen: now,
            agent: agent.to_string(),
            ip: ip.to_string(),
        });
        self.keep(&sessions);
        Ok(token)
    }

    /// The session a token belongs to, and whether its life was just
    /// extended (the cookie then goes out again).
    pub fn check(&self, token: &str, now: u64) -> Option<(String, bool)> {
        let hash = token_hash(token);
        let mut sessions = self.sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|s| s.seen + LIFE > now);
        let mut changed = sessions.len() != before;
        let found = sessions
            .iter_mut()
            .find(|s| same(s.hash.as_bytes(), hash.as_bytes()));
        let answer = found.map(|session| {
            let renew = now >= session.seen + RENEW;
            if renew {
                session.seen = now;
                changed = true;
            }
            (session.id.clone(), renew)
        });
        if changed {
            self.keep(&sessions);
        }
        answer
    }

    pub fn sessions(&self) -> Vec<Session> {
        self.sessions.lock().unwrap().clone()
    }

    /// One session, or `None` for all of them.
    pub fn sign_out(&self, id: Option<&str>) {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|s| id.is_some_and(|id| s.id != id));
        self.keep(&sessions);
        let _ = self.revoked.send(id.unwrap_or("*").to_string());
    }

    fn keep(&self, sessions: &[Session]) {
        // `--dev-open`'s session ends with the run.
        let sessions: Vec<&Session> = sessions.iter().filter(|s| s.id != "open").collect();
        if let Err(error) = save(&self.dir, "sessions.json", &sessions) {
            eprintln!("sessions.json: {error}");
        }
    }
}

fn read_secrets(dir: &Path) -> Result<Secrets, String> {
    let file = dir.join("auth.json");
    let text = std::fs::read_to_string(&file).map_err(|_| {
        format!(
            "no {}: run `plain-web init --data {}` first",
            file.display(),
            dir.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))
}

/* ------------------------------------------------------------- terminal */

fn ask_password() -> Result<String, String> {
    loop {
        let first = rpassword::prompt_password("password: ").map_err(|e| e.to_string())?;
        if first.chars().count() < 10 {
            eprintln!("at least 10 characters");
            continue;
        }
        let again = rpassword::prompt_password("again: ").map_err(|e| e.to_string())?;
        if first == again {
            return Ok(first);
        }
        eprintln!("they differ");
    }
}

fn ask(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

/// `plain-web init`: a password, a secret shown as a QR code for the
/// authenticator app, and one code from it to be sure it took.
pub fn init(dir: &Path) -> Result<(), String> {
    let _lock = lock(dir)?;
    if dir.join("auth.json").exists()
        && ask("auth.json exists: replace the password and the app's key, signing every device out? [y/N] ")?
            != "y"
    {
        return Err("nothing changed".into());
    }
    let password = ask_password()?;
    let mut raw = [0u8; 20];
    OsRng.fill_bytes(&mut raw);
    let Secret::Encoded(secret) = Secret::Raw(raw.to_vec()).to_encoded() else {
        unreachable!("to_encoded encodes");
    };
    let url = format!("otpauth://totp/Plain?secret={secret}&issuer=Plain");
    let code = qrcode::QrCode::new(&url).map_err(|e| e.to_string())?;
    // Black on white whatever the terminal's own colours: a negative is
    // what authenticator apps fail to read.
    let picture: String = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build()
        .lines()
        .map(|line| format!("\x1b[30;47m{line}\x1b[0m\n"))
        .collect();
    println!("\nScan this in Google Authenticator (or enter the key by hand):\n\n{picture}\nkey: {secret}\n");
    for _ in 0..3 {
        if let Some(step) = code_step(&secret, &ask("code from the app: ")?, now()) {
            Auth::create(dir, &password, &secret, step)?;
            println!(
                "done: {} written; every device signs in again",
                dir.join("auth.json").display()
            );
            return Ok(());
        }
        eprintln!("that code does not match — check the phone's clock, or try the next one");
    }
    Err("nothing written".into())
}

pub fn reset_password(dir: &Path) -> Result<(), String> {
    let _lock = lock(dir)?;
    read_secrets(dir)?;
    Auth::reset_password(dir, &ask_password()?)?;
    println!("done: every device signs in again");
    Ok(())
}

/* ----------------------------------------------------------------- http */

/// What the guard found, for the handlers behind it (devices, the socket).
#[derive(Clone)]
pub struct Signed {
    pub id: String,
    pub auth: Arc<Auth>,
}

/// Every `plain_session` the request carries: a stale one ahead of the
/// good one must not hide it.
fn tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// `Secure` wherever it can work: everywhere but this machine over http,
/// where Safari would drop the cookie.
fn cookie(value: &str, max_age: u64, headers: &HeaderMap) -> HeaderValue {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let name = host.rsplit_once(':').map_or(host, |(name, _)| name);
    let secure = if matches!(name, "localhost" | "127.0.0.1" | "[::1]") {
        ""
    } else {
        "; Secure"
    };
    let text =
        format!("{COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}{secure}");
    HeaderValue::from_str(&text).expect("hex and ascii")
}

/// The address Caddy saw (its last `X-Forwarded-For` hop). Without Caddy
/// every request is from this machine: the server listens on 127.0.0.1, or
/// on the Docker bridge only Caddy and its neighbours can reach.
fn ip_of(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "local".into())
}

fn agent_of(headers: &HeaderMap) -> String {
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    agent.chars().take(300).collect()
}

/// Everything but the sign-in page needs a session: the API and the socket
/// say 401, a page goes to sign in.
pub async fn guard(State(auth): State<Arc<Auth>>, mut request: Request, next: Next) -> Response {
    // The home-screen icon and the manifest are fetched without the cookie.
    if matches!(
        request.uri().path(),
        "/login"
            | "/robots.txt"
            | "/manifest.webmanifest"
            | "/icon-192.png"
            | "/icon-512.png"
            | "/apple-touch-icon.png"
    ) {
        return next.run(request).await;
    }
    let now = now();
    let found = tokens(request.headers()).find_map(|t| {
        auth.check(t, now)
            .map(|(id, renewed)| (t.to_string(), id, renewed))
    });
    let Some((token, id, renewed)) = found else {
        return if request.uri().path().starts_with("/api/") {
            StatusCode::UNAUTHORIZED.into_response()
        } else {
            Redirect::to("/login").into_response()
        };
    };
    let fresh = renewed.then(|| cookie(&token, LIFE, request.headers()));
    request.extensions_mut().insert(Signed { id, auth });
    let mut response = next.run(request).await;
    if let Some(fresh) = fresh {
        response.headers_mut().append(header::SET_COOKIE, fresh);
    }
    response
}

const STYLE: &str = "<style>
body{margin:0;background:#f7f6f3;color:#1c1b19;font:14px/1.6 ui-monospace,Menlo,Consolas,monospace}
main{max-width:520px;margin:12vh auto;padding:0 16px}
h1{font-size:18px;font-weight:600;margin:0 0 24px}
label{display:block;margin:0 0 14px;color:#6b6760}
input{display:block;width:100%;box-sizing:border-box;margin-top:4px;padding:8px;font:inherit;
color:inherit;background:#fff;border:1px solid #e6e3dc;border-radius:4px}
button{font:inherit;padding:8px 16px;border:1px solid #1c1b19;background:#1c1b19;color:#f7f6f3;
border-radius:4px;cursor:pointer;min-height:44px}
button.link{background:none;color:#1c1b19;border:none;padding:0;text-decoration:underline;min-height:0}
.error{color:#b42318;margin:0 0 16px}
table{width:100%;border-collapse:collapse;margin:0 0 24px}
td{padding:8px 8px 8px 0;border-bottom:1px solid #e6e3dc;vertical-align:top}
.muted{color:#6b6760}
a{color:inherit}
</style>";

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex, nofollow\">\
         <title>{title}</title>{STYLE}</head><body><main>{body}</main></body></html>"
    ))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Deserialize)]
pub struct LoginQuery {
    e: Option<u8>,
}

pub async fn login_page(
    State(auth): State<Arc<Auth>>,
    headers: HeaderMap,
    Query(query): Query<LoginQuery>,
) -> Response {
    if tokens(&headers).any(|t| auth.check(t, now()).is_some()) {
        return Redirect::to("/").into_response();
    }
    let error = match query.e {
        Some(1) => "<p class=\"error\">wrong password or code</p>",
        Some(2) => "<p class=\"error\">too many attempts — try again in 15 minutes</p>",
        _ => "",
    };
    page(
        "Plain — sign in",
        &format!(
            "<h1>plain</h1>{error}<form method=\"post\" action=\"/login\">\
             <label>password<input type=\"password\" name=\"password\" autocomplete=\"current-password\" required autofocus></label>\
             <label>code from the app<input name=\"code\" inputmode=\"numeric\" autocomplete=\"one-time-code\" required></label>\
             <button>sign in</button></form>"
        ),
    )
    .into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
    code: String,
}

pub async fn login(
    State(auth): State<Arc<Auth>>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    if !crate::events::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (ip, agent) = (ip_of(&headers), agent_of(&headers));
    let (who, what) = (ip.clone(), agent.clone());
    let gate = auth.clone();
    let _turn = gate.gate.lock().await;
    let attempt = tokio::task::spawn_blocking(move || {
        auth.sign_in(&form.password, &form.code, &who, &what, now())
    })
    .await;
    match attempt {
        Ok(Ok(token)) => {
            eprintln!("signed in: ip={ip} agent={agent:?}");
            let mut response = Redirect::to("/").into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, cookie(&token, LIFE, &headers));
            response
        }
        Ok(Err(Refusal::Paused)) => {
            eprintln!("sign-in refused, too many attempts: ip={ip} agent={agent:?}");
            Redirect::to("/login?e=2").into_response()
        }
        Ok(Err(Refusal::Wrong)) => {
            eprintln!("sign-in failed: ip={ip} agent={agent:?}");
            Redirect::to("/login?e=1").into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `Safari · iPhone` out of a user agent string, roughly; enough to tell
/// one's own devices apart.
fn device(agent: &str) -> String {
    let browser = [
        ("Edg/", "Edge"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ]
    .into_iter()
    .find(|(mark, _)| agent.contains(mark))
    .map_or("browser", |(_, name)| name);
    let system = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Mac OS X", "Mac"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .into_iter()
    .find(|(mark, _)| agent.contains(mark))
    .map_or("", |(_, name)| name);
    if system.is_empty() {
        browser.to_string()
    } else {
        format!("{browser} · {system}")
    }
}

fn ago(at: u64, now: u64) -> String {
    let minutes = now.saturating_sub(at) / 60;
    match minutes {
        0 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 24 * 60 => format!("{} h ago", m / 60),
        m => format!("{} d ago", m / (24 * 60)),
    }
}

pub async fn devices(Extension(signed): Extension<Signed>) -> Html<String> {
    let now = now();
    let mut sessions = signed.auth.sessions();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.seen));
    let rows: String = sessions
        .iter()
        .map(|s| {
            let action = if s.id == signed.id {
                "<span class=\"muted\">this device</span>".to_string()
            } else {
                format!(
                    "<form method=\"post\" action=\"/logout?id={}\"><button class=\"link\">sign out</button></form>",
                    escape(&s.id)
                )
            };
            format!(
                "<tr><td>{}<br><span class=\"muted\">{} · signed in {} · last used {}</span></td><td>{action}</td></tr>",
                escape(&device(&s.agent)),
                escape(&s.ip),
                ago(s.created, now),
                ago(s.seen, now)
            )
        })
        .collect();
    page(
        "Plain — devices",
        &format!(
            "<h1>devices</h1><table>{rows}</table>\
             <form method=\"post\" action=\"/logout-all\"><button>sign out everywhere</button></form>\
             <p><a href=\"/\">back to Plain</a></p>"
        ),
    )
}

#[derive(Deserialize)]
pub struct LogoutQuery {
    id: Option<String>,
}

/// One device (`?id=`), or this one. Only from this site's own pages.
pub async fn logout(
    Extension(signed): Extension<Signed>,
    headers: HeaderMap,
    Query(query): Query<LogoutQuery>,
) -> Response {
    if !crate::events::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let id = query.id.unwrap_or_else(|| signed.id.clone());
    signed.auth.sign_out(Some(&id));
    eprintln!("signed out: session={id}");
    if id == signed.id {
        signed_out(&headers)
    } else {
        Redirect::to("/devices").into_response()
    }
}

pub async fn logout_all(Extension(signed): Extension<Signed>, headers: HeaderMap) -> Response {
    if !crate::events::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    signed.auth.sign_out(None);
    eprintln!("signed out everywhere");
    signed_out(&headers)
}

fn signed_out(headers: &HeaderMap) -> Response {
    let mut response = Redirect::to("/login").into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, cookie("", 0, headers));
    response
}

impl Auth {
    /// A device signed in without a password: `--dev-open` (main.rs) and the
    /// HTTP tests. Debug builds only.
    #[cfg(debug_assertions)]
    pub fn open_session(&self) -> String {
        let token = random_hex(32);
        self.sessions.lock().unwrap().push(Session {
            id: "open".into(),
            hash: token_hash(&token),
            created: now(),
            seen: now(),
            agent: String::new(),
            ip: "local".into(),
        });
        token
    }
}

/// A device already signed in, for the HTTP tests (tests.rs).
#[cfg(test)]
pub fn signed_for_tests(dir: &Path) -> (Arc<Auth>, String) {
    Auth::create(dir, "a test password", tests::SECRET, 0).unwrap();
    let auth = Auth::load(dir).unwrap();
    let token = auth.open_session();
    (Arc::new(auth), token)
}

/// The app's code right now, for the HTTP tests.
#[cfg(test)]
pub fn code_for_tests() -> String {
    totp(tests::SECRET).unwrap().generate(now())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SECRET: &str = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    const PASSWORD: &str = "correct horse battery";

    fn auth() -> (tempfile::TempDir, Auth) {
        let dir = tempfile::tempdir().unwrap();
        Auth::create(dir.path(), PASSWORD, SECRET, 0).unwrap();
        let auth = Auth::load(dir.path()).unwrap();
        (dir, auth)
    }

    #[test]
    fn the_open_session_is_never_saved() {
        let (dir, auth) = auth();
        auth.open_session();
        auth.sign_out(Some("none"));
        assert_eq!(auth.sessions.lock().unwrap().len(), 1);
        assert!(Auth::load(dir.path())
            .unwrap()
            .sessions
            .lock()
            .unwrap()
            .is_empty());
    }

    fn code(at: u64) -> String {
        totp(SECRET).unwrap().generate(at)
    }

    #[test]
    fn rfc_6238_sha1() {
        // The RFC's key is ASCII "12345678901234567890"; 8 digits there, 6 here.
        let key = Secret::Raw(b"12345678901234567890".to_vec()).to_encoded();
        let Secret::Encoded(key) = key else {
            unreachable!()
        };
        assert_eq!(totp(&key).unwrap().generate(59), "287082");
        assert_eq!(totp(&key).unwrap().generate(1111111109), "081804");
    }

    #[test]
    fn a_code_from_a_step_either_side_counts() {
        let t = 1_800_000_000;
        assert_eq!(code_step(SECRET, &code(t - 30), t), Some(t / 30 - 1));
        assert_eq!(code_step(SECRET, &code(t + 30), t), Some(t / 30 + 1));
        assert_eq!(code_step(SECRET, &code(t - 90), t), None);
        let spaced = format!("{} {}", &code(t)[..3], &code(t)[3..]);
        assert_eq!(code_step(SECRET, &spaced, t), Some(t / 30));
    }

    #[test]
    fn signing_in_takes_both_and_a_code_only_once() {
        let (_dir, auth) = auth();
        let t = 1_800_000_000;
        assert!(matches!(
            auth.sign_in("wrong", &code(t), "1.2.3.4", "", t),
            Err(Refusal::Wrong)
        ));
        assert!(matches!(
            auth.sign_in(PASSWORD, "000000", "1.2.3.4", "", t),
            Err(Refusal::Wrong)
        ));
        let token = auth
            .sign_in(PASSWORD, &code(t), "1.2.3.4", "", t)
            .ok()
            .unwrap();
        assert!(auth.check(&token, t).is_some());
        // The same code again, even from another device.
        assert!(matches!(
            auth.sign_in(PASSWORD, &code(t), "5.6.7.8", "", t + 1),
            Err(Refusal::Wrong)
        ));
        assert!(auth
            .sign_in(PASSWORD, &code(t + 30), "5.6.7.8", "", t + 30)
            .is_ok());
    }

    #[test]
    fn five_wrong_tries_and_that_place_waits() {
        let (_dir, auth) = auth();
        let t = 1_800_000_000;
        for i in 0..5 {
            assert!(matches!(
                auth.sign_in("wrong", "1", "1.2.3.4", "", t + i),
                Err(Refusal::Wrong)
            ));
        }
        assert!(matches!(
            auth.sign_in(PASSWORD, &code(t + 10), "1.2.3.4", "", t + 10),
            Err(Refusal::Paused)
        ));
        // Somewhere else is not paused.
        assert!(auth
            .sign_in(PASSWORD, &code(t + 10), "9.9.9.9", "", t + 10)
            .is_ok());
        // Fifteen minutes after the last wrong try.
        let later = t + 4 + PAUSE;
        assert!(auth
            .sign_in(PASSWORD, &code(later), "1.2.3.4", "", later)
            .is_ok());
    }

    #[test]
    fn a_whole_ipv6_64_is_one_place() {
        let (_dir, auth) = auth();
        let t = 1_800_000_000;
        for i in 0..5 {
            let ip = format!("2001:db8:1:2::{i:x}");
            assert!(matches!(
                auth.sign_in("wrong", "1", &ip, "", t),
                Err(Refusal::Wrong)
            ));
        }
        assert!(matches!(
            auth.sign_in(PASSWORD, &code(t), "2001:db8:1:2::ff", "", t),
            Err(Refusal::Paused)
        ));
    }

    #[test]
    fn ipv4_in_ipv6_clothing_is_its_own_place() {
        assert_eq!(place("::ffff:1.2.3.4"), "1.2.3.4");
        assert_ne!(place("::ffff:1.2.3.4"), place("::ffff:5.6.7.8"));
    }

    #[test]
    fn tries_sent_all_at_once_are_counted() {
        let (_dir, auth) = auth();
        let auth = Arc::new(auth);
        let t = 1_800_000_000;
        let tries: Vec<_> = (0..8)
            .map(|_| {
                let auth = auth.clone();
                std::thread::spawn(move || auth.sign_in("wrong", "1", "1.2.3.4", "", t))
            })
            .collect();
        let wrong = tries
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|answer| matches!(answer, Err(Refusal::Wrong)))
            .count();
        assert_eq!(wrong, TRIES as usize);
    }

    #[test]
    fn one_server_or_one_command_per_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let held = lock(dir.path()).unwrap();
        assert!(lock(dir.path()).is_err());
        drop(held);
        assert!(lock(dir.path()).is_ok());
    }

    #[test]
    fn guessing_codes_with_the_password_stops_everyone() {
        let (_dir, auth) = auth();
        let t = 1_800_000_000;
        for i in 0..NEAR_TRIES {
            let ip = format!("10.0.{i}.1");
            assert!(matches!(
                auth.sign_in(PASSWORD, "000000", &ip, "", t),
                Err(Refusal::Wrong)
            ));
        }
        assert!(matches!(
            auth.sign_in(PASSWORD, &code(t), "10.9.9.9", "", t),
            Err(Refusal::Paused)
        ));
    }

    #[test]
    fn a_session_lives_thirty_days_from_its_last_use() {
        let (dir, auth) = auth();
        let t = 1_800_000_000;
        let token = auth
            .sign_in(PASSWORD, &code(t), "1.2.3.4", "", t)
            .ok()
            .unwrap();
        // Within the hour: nothing written.
        assert_eq!(auth.check(&token, t + 60).map(|s| s.1), Some(false));
        assert_eq!(auth.check(&token, t + RENEW).map(|s| s.1), Some(true));
        let seen = auth.sessions()[0].seen;
        assert_eq!(seen, t + RENEW);
        assert!(auth.check(&token, seen + LIFE - 1).is_some());
        assert!(auth.check(&token, seen + LIFE - 1 + LIFE).is_none());
        assert!(auth.sessions().is_empty());
        // It is on disk, by hash only.
        let file = std::fs::read_to_string(dir.path().join("sessions.json")).unwrap();
        assert!(!file.contains(&token));
    }

    #[test]
    fn signing_out_one_or_all() {
        let (_dir, auth) = auth();
        let t = 1_800_000_000;
        let a = auth
            .sign_in(PASSWORD, &code(t), "1.1.1.1", "", t)
            .ok()
            .unwrap();
        let b = auth
            .sign_in(PASSWORD, &code(t + 30), "2.2.2.2", "", t + 30)
            .ok()
            .unwrap();
        let mut revoked = auth.revoked.subscribe();
        let (a_id, _) = auth.check(&a, t + 30).unwrap();
        auth.sign_out(Some(&a_id));
        assert_eq!(revoked.try_recv().unwrap(), a_id);
        assert!(auth.check(&a, t + 30).is_none());
        assert!(auth.check(&b, t + 30).is_some());
        auth.sign_out(None);
        assert_eq!(revoked.try_recv().unwrap(), "*");
        assert!(auth.check(&b, t + 30).is_none());
    }

    #[test]
    fn reset_password_keeps_the_app_and_drops_the_devices() {
        let (dir, auth) = auth();
        let t = 1_800_000_000;
        auth.sign_in(PASSWORD, &code(t), "1.1.1.1", "", t)
            .ok()
            .unwrap();
        Auth::reset_password(dir.path(), "a brand new one").unwrap();
        let auth = Auth::load(dir.path()).unwrap();
        assert!(auth.sessions().is_empty());
        assert!(auth
            .sign_in(PASSWORD, &code(t + 60), "1.1.1.1", "", t + 60)
            .is_err());
        assert!(auth
            .sign_in("a brand new one", &code(t + 90), "1.1.1.1", "", t + 90)
            .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn the_secrets_are_the_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, _auth) = auth();
        for name in ["auth.json", "sessions.json"] {
            let mode = std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name}");
        }
    }

    #[test]
    fn devices_have_names() {
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(device(safari), "Safari · iPhone");
        let chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
        assert_eq!(device(chrome), "Chrome · Mac");
    }
}
