#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod agent;
mod tunnel;
mod wiki;

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Multipart, Path, Request, State,
    },
    http::{header, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use rusqlite::{params, types::ValueRef, Connection};
use rust_embed::RustEmbed;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::{process::Command, sync::broadcast, task::AbortHandle};

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

pub struct App {
    pub db: Mutex<Connection>,
    pub tx: broadcast::Sender<String>,
    pub token: String,
    pub dir: PathBuf,
    pub running: Mutex<HashMap<i64, AbortHandle>>,
    pub http: reqwest::Client,
    pub ntfy_server: String,
    pub ntfy_topic: String,
    pub public_url: Mutex<String>,
    pub tunnel_restart: tokio::sync::Notify,
}
pub type S = Arc<App>;

pub struct AppErr(pub StatusCode, pub String);
impl AppErr {
    pub fn new(code: StatusCode, msg: impl Into<String>) -> Self {
        Self(code, msg.into())
    }
}
impl IntoResponse for AppErr {
    fn into_response(self) -> Response {
        let body = json!({
            "statusCode": self.0.as_u16(),
            "error": self.0.canonical_reason().unwrap_or(""),
            "message": self.1,
        });
        (self.0, Json(body)).into_response()
    }
}
impl From<rusqlite::Error> for AppErr {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => Self::new(StatusCode::NOT_FOUND, "not found"),
            _ => {
                eprintln!("db error: {e}");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }
    }
}
type R<T = Value> = Result<Json<T>, AppErr>;

impl App {
    pub fn db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, AppErr> {
        let c = self.db.lock().unwrap();
        Ok(f(&c)?)
    }

    pub fn changed(&self) {
        let _ = self.tx.send(r#"{"t":"changed"}"#.into());
    }

    /// Notifies connected clients and sends a push (ntfy) to the Android app.
    pub fn push(self: &Arc<Self>, title: &str, body: &str) {
        self.changed();
        let a = self.clone();
        let payload = json!({"topic": a.ntfy_topic, "title": title, "message": body});
        tokio::spawn(async move {
            if let Err(e) = a.http.post(&a.ntfy_server).json(&payload).send().await {
                eprintln!("push failed: {e}");
            }
        });
    }

    pub fn card_title(&self, id: i64) -> String {
        self.db(|c| c.query_row("SELECT title FROM cards WHERE id=?", [id], |r| r.get(0)))
            .unwrap_or_default()
    }
}

pub fn rows(c: &Connection, sql: &str, p: impl rusqlite::Params) -> rusqlite::Result<Vec<Value>> {
    let mut st = c.prepare(sql)?;
    let names: Vec<String> = st.column_names().into_iter().map(|s| s.to_string()).collect();
    let it = st.query_map(p, |r| {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            let v = match r.get_ref(i)? {
                ValueRef::Integer(x) => json!(x),
                ValueRef::Real(x) => json!(x),
                ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                _ => Value::Null,
            };
            m.insert(n.clone(), v);
        }
        Ok(Value::Object(m))
    })?;
    it.collect()
}

fn str_of<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}
fn int_of(v: &Value, k: &str) -> Result<i64, AppErr> {
    v.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| AppErr::new(StatusCode::BAD_REQUEST, format!("missing {k}")))
}
fn need<'a>(v: &'a Value, k: &str) -> Result<&'a str, AppErr> {
    str_of(v, k)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppErr::new(StatusCode::BAD_REQUEST, format!("missing {k}")))
}
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

const SCHEMA: &str = "
PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS projects(id INTEGER PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS lanes(id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects ON DELETE CASCADE, name TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'free', pos REAL NOT NULL);
CREATE TABLE IF NOT EXISTS cards(id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects ON DELETE CASCADE, lane_id INTEGER NOT NULL REFERENCES lanes, title TEXT NOT NULL, body TEXT NOT NULL DEFAULT '', context TEXT NOT NULL DEFAULT '', agent TEXT NOT NULL DEFAULT 'claude', model TEXT NOT NULL DEFAULT 'sonnet', status TEXT NOT NULL DEFAULT 'idle', pos REAL NOT NULL);
CREATE TABLE IF NOT EXISTS checklist(id INTEGER PRIMARY KEY, card_id INTEGER NOT NULL REFERENCES cards ON DELETE CASCADE, text TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS messages(id INTEGER PRIMARY KEY, card_id INTEGER NOT NULL REFERENCES cards ON DELETE CASCADE, role TEXT NOT NULL, content TEXT NOT NULL, ts INTEGER NOT NULL DEFAULT (unixepoch()));
CREATE TABLE IF NOT EXISTS attachments(id INTEGER PRIMARY KEY, card_id INTEGER NOT NULL REFERENCES cards ON DELETE CASCADE, name TEXT NOT NULL, kind TEXT NOT NULL, path TEXT, text TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS settings(k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE VIRTUAL TABLE IF NOT EXISTS rag USING fts5(content, card_id UNINDEXED, project_id UNINDEXED);
";

const DEFAULT_LANES: [(&str, &str); 6] = [
    ("Backlog", "free"),
    ("Refinar", "refine"),
    ("Refinado", "refine_done"),
    ("Desenvolver", "create"),
    ("Q.A", "qa"),
    ("Done", "done"),
];

fn home_dir() -> String {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default()
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random generator");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn setting(c: &Connection, k: &str, default: impl FnOnce() -> String) -> rusqlite::Result<String> {
    if let Ok(v) = c.query_row("SELECT v FROM settings WHERE k=?", [k], |r| r.get(0)) {
        return Ok(v);
    }
    let v = default();
    c.execute("INSERT INTO settings(k,v) VALUES(?,?)", params![k, v])?;
    Ok(v)
}

fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("8.8.8.8:80").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |x, (p, q)| x | (p ^ q)) == 0
}

async fn auth(State(a): State<S>, req: Request, next: Next) -> Response {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let query = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("token=")));
    if bearer.or(query).is_some_and(|t| ct_eq(t, &a.token)) {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

// ---------- state ----------

async fn state(State(a): State<S>) -> R {
    a.db(|c| {
        Ok(json!({
            "projects": rows(c, "SELECT id,name,path,wiki_status,wiki_files FROM projects ORDER BY id", [])?,
            "lanes": rows(c, "SELECT * FROM lanes ORDER BY pos", [])?,
            "cards": rows(c, "SELECT c.id,c.project_id,c.lane_id,c.title,c.body,c.agent,c.model,c.status,c.pos,c.archived,
                (SELECT COUNT(*) FROM checklist WHERE card_id=c.id) total,
                (SELECT COUNT(*) FROM checklist WHERE card_id=c.id AND done=1) done,
                (SELECT COUNT(*) FROM attachments WHERE card_id=c.id) files
                FROM cards c ORDER BY pos", [])?,
            "settings": {
                "ntfy_server": a.ntfy_server,
                "ntfy_topic": a.ntfy_topic,
                "public_url": *a.public_url.lock().unwrap(),
                "pair": json!({"s": a.ntfy_server, "t": a.ntfy_topic, "k": a.token}).to_string(),
            },
        }))
    })
    .map(Json)
}

// ---------- CLI accounts ----------

async fn cli_output(bin: &str, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(std::process::Stdio::null()).kill_on_drop(true);
    agent::scrub_env(&mut cmd);
    let out = tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await.ok()?.ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text.trim().to_string()))
}

async fn auth_status() -> R {
    let (claude, codex) = tokio::join!(cli_output("claude", &["auth", "status"]), cli_output("codex", &["login", "status"]));
    let claude = match claude {
        None => json!({"installed": false, "logged_in": false}),
        Some((_, t)) => {
            let v: Value = serde_json::from_str(&t).unwrap_or_default();
            json!({"installed": true, "logged_in": v["loggedIn"] == true, "detail": v["email"].as_str().unwrap_or("")})
        }
    };
    let codex = match codex {
        None => json!({"installed": false, "logged_in": false}),
        Some((ok, t)) => json!({"installed": true, "logged_in": ok && t.contains("Logged in"), "detail": t.lines().last().unwrap_or("")}),
    };
    Ok(Json(json!({"claude": claude, "codex": codex})))
}

/// Starts the CLI's own OAuth login; it opens the browser on this Mac.
async fn auth_login(State(a): State<S>, Path(who): Path<String>) -> Result<StatusCode, AppErr> {
    let (bin, args): (&str, &[&str]) = match who.as_str() {
        "claude" => ("claude", &["auth", "login"]),
        "codex" => ("codex", &["login"]),
        _ => return Err(AppErr::new(StatusCode::NOT_FOUND, "unknown agent")),
    };
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    agent::scrub_env(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, format!("não foi possível iniciar {bin}: {e}")))?;
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(300), child.wait()).await;
        a.push("Login concluído", bin);
    });
    Ok(StatusCode::ACCEPTED)
}

// ---------- tunnel setup ----------

/// Opens a documentation/sign-up page in the user's default browser (the webview can't open new windows).
async fn open_url(Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let url = need(&b, "url")?;
    if !(url.starts_with("https://dashboard.ngrok.com/") || url.starts_with("https://ngrok.com/")) {
        return Err(AppErr::new(StatusCode::BAD_REQUEST, "url not allowed"));
    }
    let mut cmd = match std::env::consts::OS {
        "macos" => Command::new("open"),
        "windows" => {
            let mut c = Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        }
        _ => Command::new("xdg-open"),
    };
    cmd.arg(url).spawn().map_err(io_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn tunnel_status(State(a): State<S>) -> R {
    Ok(Json(tunnel::status(&a).await))
}

async fn tunnel_ngrok(State(a): State<S>, Json(b): Json<Value>) -> R {
    tunnel::save_authtoken(&a, need(&b, "authtoken")?.trim())
        .await
        .map_err(|e| AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(tunnel::status(&a).await))
}

async fn tunnel_restart(State(a): State<S>) -> StatusCode {
    a.tunnel_restart.notify_one();
    StatusCode::ACCEPTED
}

// ---------- models & folder browser ----------

async fn models() -> R {
    let home = home_dir();
    // Codex keeps the models available to the logged-in account in its cache.
    let codex: Vec<String> = std::fs::read_to_string(format!("{home}/.codex/models_cache.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| {
            v["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["visibility"] == "list")
                .filter_map(|m| m["slug"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "claude": ["sonnet", "opus", "haiku", "claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
        "codex": codex,
    })))
}

async fn browse(axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>) -> R {
    let home = home_dir();
    let path = q.get("path").filter(|p| !p.is_empty()).cloned().unwrap_or(home);
    let dir = std::fs::canonicalize(&path).map_err(|_| AppErr::new(StatusCode::NOT_FOUND, "folder not found"))?;
    let mut dirs: Vec<String> = std::fs::read_dir(&dir)
        .map_err(io_err)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    dirs.sort_by_key(|n| n.to_lowercase());
    Ok(Json(json!({
        "path": dir.to_string_lossy(),
        "parent": dir.parent().map(|p| p.to_string_lossy().to_string()),
        "dirs": dirs,
    })))
}

async fn mkdir(Json(b): Json<Value>) -> R {
    let (parent, name) = (need(&b, "path")?, need(&b, "name")?);
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(AppErr::new(StatusCode::BAD_REQUEST, "invalid folder name"));
    }
    let dir = std::path::Path::new(parent).join(name);
    std::fs::create_dir(&dir).map_err(|e| AppErr::new(StatusCode::CONFLICT, e.to_string()))?;
    Ok(Json(json!({ "path": dir.to_string_lossy() })))
}

// ---------- projects & lanes ----------

async fn new_project(State(a): State<S>, Json(b): Json<Value>) -> R {
    let (name, path) = (need(&b, "name")?, need(&b, "path")?);
    if !std::path::Path::new(path).is_dir() {
        return Err(AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, "path is not a directory"));
    }
    let id = a.db(|c| {
        c.execute("INSERT INTO projects(name,path) VALUES(?,?)", params![name, path])?;
        let id = c.last_insert_rowid();
        for (i, (n, k)) in DEFAULT_LANES.iter().enumerate() {
            c.execute(
                "INSERT INTO lanes(project_id,name,kind,pos) VALUES(?,?,?,?)",
                params![id, n, k, i as f64],
            )?;
        }
        Ok(id)
    })?;
    a.push("Projeto criado", name);
    index_project(&a, id);
    Ok(Json(json!({ "id": id })))
}

/// Scans the project folder in the background and rebuilds its wiki (RAG, card_id=0).
fn index_project(a: &S, id: i64) {
    let a = a.clone();
    tokio::spawn(async move {
        let Ok(path) = a.db(|c| c.query_row("SELECT path FROM projects WHERE id=?", [id], |r| r.get::<_, String>(0))) else { return };
        if a.db(|c| c.execute("UPDATE projects SET wiki_status='indexing' WHERE id=? AND wiki_status!='indexing'", [id])).map_or(0, |n| n) == 0 {
            return;
        }
        a.changed();
        let scan = tokio::task::spawn_blocking(move || wiki::build(std::path::Path::new(&path))).await;
        let result = match scan {
            Ok(Ok(scan)) => a.db(|c| {
                let tx = c.unchecked_transaction()?;
                tx.execute("DELETE FROM rag WHERE project_id=? AND card_id=0", [id])?;
                for chunk in &scan.chunks {
                    tx.execute("INSERT INTO rag(content,card_id,project_id) VALUES(?,0,?)", params![chunk, id])?;
                }
                tx.execute(
                    "UPDATE projects SET wiki=?,wiki_status='ready',wiki_files=? WHERE id=?",
                    params![scan.overview, scan.files as i64, id],
                )?;
                tx.commit()
            }).map_err(|e| e.1),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e.to_string()),
        };
        if let Err(e) = result {
            eprintln!("wiki indexing failed for project {id}: {e}");
            let _ = a.db(|c| c.execute("UPDATE projects SET wiki_status='error' WHERE id=?", [id]));
            a.push("Falha ao indexar projeto", &e);
        } else {
            a.push("Wiki do projeto pronta", &a.db(|c| c.query_row("SELECT name FROM projects WHERE id=?", [id], |r| r.get::<_, String>(0))).unwrap_or_default());
        }
    });
}

async fn reindex(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    a.db(|c| c.query_row("SELECT 1 FROM projects WHERE id=?", [id], |_| Ok(())))?;
    index_project(&a, id);
    Ok(StatusCode::ACCEPTED)
}

async fn del_project(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    let ids = a.running.lock().unwrap().keys().copied().collect::<Vec<_>>();
    for card in ids {
        agent::stop(&a, card);
    }
    a.db(|c| {
        c.execute("DELETE FROM rag WHERE project_id=?", [id])?;
        c.execute("DELETE FROM projects WHERE id=?", [id]).map(|_| ())
    })?;
    a.push("Projeto removido", &format!("#{id}"));
    Ok(StatusCode::NO_CONTENT)
}

async fn new_lane(State(a): State<S>, Json(b): Json<Value>) -> R {
    let (pid, name) = (int_of(&b, "project_id")?, need(&b, "name")?);
    let id = a.db(|c| {
        c.execute(
            "INSERT INTO lanes(project_id,name,kind,pos) SELECT ?,?, 'free', COALESCE(MAX(pos),0)+1 FROM lanes WHERE project_id=?",
            params![pid, name, pid],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    a.push("Categoria criada", name);
    Ok(Json(json!({ "id": id })))
}

async fn patch_lane(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let name = need(&b, "name")?;
    a.db(|c| c.execute("UPDATE lanes SET name=? WHERE id=?", params![name, id]).map(|_| ()))?;
    a.push("Categoria renomeada", name);
    Ok(StatusCode::NO_CONTENT)
}

async fn del_lane(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    let (kind, cards): (String, i64) = a.db(|c| {
        c.query_row(
            "SELECT kind,(SELECT COUNT(*) FROM cards WHERE lane_id=lanes.id) FROM lanes WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    })?;
    if kind != "free" {
        return Err(AppErr::new(StatusCode::CONFLICT, "built-in categories cannot be deleted"));
    }
    if cards > 0 {
        return Err(AppErr::new(StatusCode::CONFLICT, "category still has cards"));
    }
    a.db(|c| c.execute("DELETE FROM lanes WHERE id=?", [id]).map(|_| ()))?;
    a.push("Categoria removida", &format!("#{id}"));
    Ok(StatusCode::NO_CONTENT)
}

// ---------- cards ----------

async fn new_card(State(a): State<S>, Json(b): Json<Value>) -> R {
    let (lane, title) = (int_of(&b, "lane_id")?, need(&b, "title")?);
    let body = str_of(&b, "body").unwrap_or("");
    let (id, kind) = a.db(|c| {
        let (pid, kind): (i64, String) =
            c.query_row("SELECT project_id,kind FROM lanes WHERE id=?", [lane], |r| Ok((r.get(0)?, r.get(1)?)))?;
        c.execute(
            "INSERT INTO cards(project_id,lane_id,title,body,pos) VALUES(?,?,?,?,?)",
            params![pid, lane, title, body, now()],
        )?;
        Ok((c.last_insert_rowid(), kind))
    })?;
    a.push("Card criado", title);
    if agent::runs_on(&kind) {
        agent::start(&a, id)?;
    }
    Ok(Json(json!({ "id": id })))
}

async fn get_card(State(a): State<S>, Path(id): Path<i64>) -> R {
    a.db(|c| {
        let card = rows(c, "SELECT c.*, l.kind lane_kind, l.name lane_name FROM cards c JOIN lanes l ON l.id=c.lane_id WHERE c.id=?", [id])?
            .pop()
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        Ok(json!({
            "card": card,
            "messages": rows(c, "SELECT id,role,content,ts FROM messages WHERE card_id=? ORDER BY id", [id])?,
            "checklist": rows(c, "SELECT id,text,done FROM checklist WHERE card_id=? ORDER BY id", [id])?,
            "attachments": rows(c, "SELECT id,name,kind,text FROM attachments WHERE card_id=? ORDER BY id", [id])?,
        }))
    })
    .map(Json)
}

async fn patch_card(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    a.db(|c| {
        for k in ["title", "body", "context", "agent", "model"] {
            if let Some(v) = str_of(&b, k) {
                c.execute(&format!("UPDATE cards SET {k}=? WHERE id=?"), params![v, id])?;
            }
        }
        if let Some(ctx) = str_of(&b, "context") {
            let pid: i64 = c.query_row("SELECT project_id FROM cards WHERE id=?", [id], |r| r.get(0))?;
            agent::index(c, pid, id, ctx)?;
        }
        Ok(())
    })?;
    a.push("Card atualizado", &a.card_title(id));
    Ok(StatusCode::NO_CONTENT)
}

async fn del_card(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    agent::stop(&a, id);
    let title = a.card_title(id);
    a.db(|c| {
        c.execute("DELETE FROM rag WHERE card_id=?", [id])?;
        c.execute("DELETE FROM cards WHERE id=?", [id]).map(|_| ())
    })?;
    a.push("Card removido", &title);
    Ok(StatusCode::NO_CONTENT)
}

async fn move_card(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let lane = int_of(&b, "lane_id")?;
    let pos = b.get("pos").and_then(Value::as_f64).unwrap_or_else(now);
    agent::stop(&a, id);
    let (kind, lane_name) = a.db(|c| {
        let r = c.query_row("SELECT kind,name FROM lanes WHERE id=?", [lane], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        c.execute("UPDATE cards SET lane_id=?,pos=?,status='idle' WHERE id=?", params![lane, pos, id])?;
        Ok(r)
    })?;
    a.push("Card movido", &format!("{} → {}", a.card_title(id), lane_name));
    if agent::runs_on(&kind) {
        agent::start(&a, id)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn archive(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let archived = b.get("archived").and_then(Value::as_bool).unwrap_or(true);
    if archived {
        agent::stop(&a, id);
    }
    let n = a.db(|c| c.execute("UPDATE cards SET archived=?,status='idle' WHERE id=?", params![archived as i64, id]))?;
    if n == 0 {
        return Err(AppErr::new(StatusCode::NOT_FOUND, "not found"));
    }
    a.push(if archived { "Card arquivado" } else { "Card restaurado" }, &a.card_title(id));
    Ok(StatusCode::NO_CONTENT)
}

/// Opens the card's project folder in Finder or VS Code on this Mac.
async fn open_dir(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let path: String = a.db(|c| {
        c.query_row("SELECT p.path FROM cards c JOIN projects p ON p.id=c.project_id WHERE c.id=?", [id], |r| r.get(0))
    })?;
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, "project folder no longer exists"));
    }
    let (bin, args): (String, Vec<String>) = match (str_of(&b, "app"), std::env::consts::OS) {
        (Some("finder"), "macos") => ("open".into(), vec![path]),
        (Some("finder"), "windows") => ("explorer".into(), vec![path]),
        (Some("finder"), _) => ("xdg-open".into(), vec![path]),
        (Some("vscode"), "macos") => ("open".into(), vec!["-a".into(), "Visual Studio Code".into(), path]),
        (Some("vscode"), "windows") => {
            let exe = format!("{}\\Programs\\Microsoft VS Code\\Code.exe", std::env::var("LOCALAPPDATA").unwrap_or_default());
            (exe, vec![path])
        }
        (Some("vscode"), _) => ("code".into(), vec![path]),
        _ => return Err(AppErr::new(StatusCode::BAD_REQUEST, "app must be finder or vscode")),
    };
    let out = Command::new(&bin)
        .args(&args)
        .output()
        .await
        .map_err(|e| AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, format!("não foi possível abrir com {bin}: {e}")))?;
    // explorer.exe reports exit code 1 even on success
    if !out.status.success() && bin != "explorer" {
        return Err(AppErr::new(StatusCode::UNPROCESSABLE_ENTITY, String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn chat(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let text = need(&b, "text")?;
    if a.running.lock().unwrap().contains_key(&id) {
        return Err(AppErr::new(StatusCode::CONFLICT, "agent is already working on this card"));
    }
    a.db(|c| {
        c.query_row("SELECT 1 FROM cards WHERE id=?", [id], |_| Ok(()))?;
        c.execute("INSERT INTO messages(card_id,role,content) VALUES(?,?,?)", params![id, "user", text])
            .map(|_| ())
    })?;
    a.push("Mensagem enviada", &format!("{}: {}", a.card_title(id), text.chars().take(120).collect::<String>()));
    agent::start(&a, id)?;
    Ok(StatusCode::ACCEPTED)
}

async fn stop(State(a): State<S>, Path(id): Path<i64>) -> StatusCode {
    agent::stop(&a, id);
    a.push("Agente interrompido", &a.card_title(id));
    StatusCode::NO_CONTENT
}

// ---------- checklist ----------

async fn add_item(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> R {
    let text = need(&b, "text")?;
    let item = a.db(|c| {
        c.execute("INSERT INTO checklist(card_id,text) VALUES(?,?)", params![id, text])?;
        Ok(c.last_insert_rowid())
    })?;
    a.push("Item adicionado", &format!("{}: {}", a.card_title(id), text));
    Ok(Json(json!({ "id": item })))
}

async fn patch_item(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> Result<StatusCode, AppErr> {
    let card = a.db(|c| {
        if let Some(done) = b.get("done").and_then(Value::as_bool) {
            c.execute("UPDATE checklist SET done=? WHERE id=?", params![done as i64, id])?;
        }
        if let Some(t) = str_of(&b, "text") {
            c.execute("UPDATE checklist SET text=? WHERE id=?", params![t, id])?;
        }
        c.query_row("SELECT card_id FROM checklist WHERE id=?", [id], |r| r.get::<_, i64>(0))
    })?;
    a.push("Checklist atualizado", &a.card_title(card));
    Ok(StatusCode::NO_CONTENT)
}

async fn del_item(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    let card = a.db(|c| {
        let card = c.query_row("SELECT card_id FROM checklist WHERE id=?", [id], |r| r.get::<_, i64>(0))?;
        c.execute("DELETE FROM checklist WHERE id=?", [id])?;
        Ok(card)
    })?;
    a.push("Item removido", &a.card_title(card));
    Ok(StatusCode::NO_CONTENT)
}

// ---------- attachments ----------

fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("file");
    let s: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    if s.trim_matches('.').is_empty() { "file".into() } else { s }
}

async fn run_capture(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn transcribe(path: &std::path::Path) -> String {
    let home = home_dir();
    let model = std::env::var("WHISPER_MODEL").unwrap_or_else(|_| format!("{home}/.agentboard/ggml-base.bin"));
    if !std::path::Path::new(&model).exists() {
        return format!("[áudio não transcrito: modelo whisper ausente em {model}; defina WHISPER_MODEL]");
    }
    let wav = format!("{}.16k.wav", path.display());
    let src = path.to_string_lossy();
    let text = match run_capture("ffmpeg", &["-y", "-loglevel", "error", "-i", &src, "-ar", "16000", "-ac", "1", &wav]).await {
        Some(_) => run_capture("whisper-cli", &["-m", &model, "-f", &wav, "-nt", "-l", "auto"]).await,
        None => None,
    };
    let _ = tokio::fs::remove_file(&wav).await;
    text.filter(|t| !t.is_empty()).unwrap_or_else(|| "[falha ao transcrever o áudio]".into())
}

async fn upload(State(a): State<S>, Path(id): Path<i64>, mut mp: Multipart) -> R {
    a.db(|c| c.query_row("SELECT 1 FROM cards WHERE id=?", [id], |_| Ok(())))?;
    let bad = |e: axum::extract::multipart::MultipartError| AppErr::new(StatusCode::BAD_REQUEST, e.to_string());
    let mut out = Vec::new();
    while let Some(field) = mp.next_field().await.map_err(bad)? {
        let name = safe_name(field.file_name().unwrap_or("file"));
        let mime = field.content_type().unwrap_or("").to_string();
        let data = field.bytes().await.map_err(bad)?;
        let dir = a.dir.join("files").join(id.to_string());
        tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
        let path = dir.join(format!("{}-{}", random_hex(4), name));
        tokio::fs::write(&path, &data).await.map_err(io_err)?;
        let (kind, text) = if mime.starts_with("image/") {
            ("image", String::new())
        } else if mime.starts_with("audio/") || mime.starts_with("video/") {
            ("audio", transcribe(&path).await)
        } else if let Ok(t) = std::str::from_utf8(&data) {
            ("text", t.chars().take(100_000).collect())
        } else {
            ("file", String::new())
        };
        let att = a.db(|c| {
            c.execute(
                "INSERT INTO attachments(card_id,name,kind,path,text) VALUES(?,?,?,?,?)",
                params![id, name, kind, path.to_string_lossy(), text],
            )?;
            Ok(json!({"id": c.last_insert_rowid(), "name": name, "kind": kind, "text": text}))
        })?;
        out.push(att);
    }
    a.push("Anexo adicionado", &a.card_title(id));
    Ok(Json(json!(out)))
}

fn io_err(e: std::io::Error) -> AppErr {
    eprintln!("io error: {e}");
    AppErr::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

fn html_to_text(html: &str) -> String {
    let drop_blocks = regex::Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>").unwrap();
    let tags = regex::Regex::new(r"(?s)<[^>]+>").unwrap();
    let ws = regex::Regex::new(r"\s+").unwrap();
    let without_blocks = drop_blocks.replace_all(html, " ");
    let t = tags.replace_all(&without_blocks, " ");
    ws.replace_all(&t, " ").trim().chars().take(20_000).collect()
}

async fn add_url(State(a): State<S>, Path(id): Path<i64>, Json(b): Json<Value>) -> R {
    let url = need(&b, "url")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppErr::new(StatusCode::BAD_REQUEST, "only http(s) urls"));
    }
    let text = match a.http.get(url).timeout(std::time::Duration::from_secs(20)).send().await {
        Ok(r) => html_to_text(&r.text().await.unwrap_or_default()),
        Err(e) => format!("[falha ao buscar a página: {e}]"),
    };
    let att = a.db(|c| {
        c.execute(
            "INSERT INTO attachments(card_id,name,kind,path,text) VALUES(?,?,?,?,?)",
            params![id, url, "url", Option::<String>::None, text],
        )?;
        Ok(json!({"id": c.last_insert_rowid(), "name": url, "kind": "url"}))
    })?;
    a.push("Link adicionado", &a.card_title(id));
    Ok(Json(att))
}

async fn del_att(State(a): State<S>, Path(id): Path<i64>) -> Result<StatusCode, AppErr> {
    let (card, path): (i64, Option<String>) =
        a.db(|c| c.query_row("SELECT card_id,path FROM attachments WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?))))?;
    if let Some(p) = path {
        let _ = tokio::fs::remove_file(p).await;
    }
    a.db(|c| c.execute("DELETE FROM attachments WHERE id=?", [id]).map(|_| ()))?;
    a.push("Anexo removido", &a.card_title(card));
    Ok(StatusCode::NO_CONTENT)
}

async fn file(State(a): State<S>, Path(id): Path<i64>) -> Result<Response, AppErr> {
    let (name, path): (String, Option<String>) =
        a.db(|c| c.query_row("SELECT name,path FROM attachments WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?))))?;
    let path = path.ok_or_else(|| AppErr::new(StatusCode::NOT_FOUND, "no file"))?;
    let data = tokio::fs::read(path).await.map_err(io_err)?;
    let mime = mime_guess::from_path(&name).first_or_octet_stream().to_string();
    Ok(([(header::CONTENT_TYPE, mime)], data).into_response())
}

// ---------- realtime & static ----------

async fn ws(State(a): State<S>, up: WebSocketUpgrade) -> Response {
    up.on_upgrade(move |s| pump(a, s))
}

async fn pump(a: S, mut s: WebSocket) {
    let mut rx = a.tx.subscribe();
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Ok(m) => if s.send(Message::Text(m.into())).await.is_err() { break },
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            },
            c = s.recv() => if !matches!(c, Some(Ok(m)) if !matches!(m, Message::Close(_))) { break },
        }
    }
}

async fn static_files(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let (name, f) = match Assets::get(path) {
        Some(f) if !path.is_empty() => (path, f),
        _ => match Assets::get("index.html") {
            Some(f) => ("index.html", f),
            None => return (StatusCode::NOT_FOUND, "ui not built").into_response(),
        },
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(f.data.into_owned()))
        .unwrap()
}

fn router(a: S) -> Router {
    let api = Router::new()
        .route("/state", get(state))
        .route("/models", get(models))
        .route("/open-url", post(open_url))
        .route("/tunnel", get(tunnel_status))
        .route("/tunnel/ngrok", post(tunnel_ngrok))
        .route("/tunnel/restart", post(tunnel_restart))
        .route("/auth", get(auth_status))
        .route("/auth/{who}/login", post(auth_login))
        .route("/fs", get(browse))
        .route("/fs/mkdir", post(mkdir))
        .route("/projects", post(new_project))
        .route("/projects/{id}", delete(del_project))
        .route("/projects/{id}/reindex", post(reindex))
        .route("/lanes", post(new_lane))
        .route("/lanes/{id}", patch(patch_lane).delete(del_lane))
        .route("/cards", post(new_card))
        .route("/cards/{id}", get(get_card).patch(patch_card).delete(del_card))
        .route("/cards/{id}/move", post(move_card))
        .route("/cards/{id}/chat", post(chat))
        .route("/cards/{id}/archive", post(archive))
        .route("/cards/{id}/open", post(open_dir))
        .route("/cards/{id}/stop", post(stop))
        .route("/cards/{id}/checklist", post(add_item))
        .route("/cards/{id}/files", post(upload).layer(DefaultBodyLimit::max(200 * 1024 * 1024)))
        .route("/cards/{id}/url", post(add_url))
        .route("/checklist/{id}", patch(patch_item).delete(del_item))
        .route("/attachments/{id}", delete(del_att));
    Router::new()
        .nest("/api", api)
        .route("/files/{id}", get(file))
        .route("/ws", get(ws))
        .layer(middleware::from_fn_with_state(a.clone(), auth))
        .fallback(static_files)
        .with_state(a)
}

/// GUI launches (Dock/Finder) get a minimal PATH: borrow the login shell's PATH and scan the usual
/// install dirs so CLIs installed through nvm/Homebrew/etc. (claude, codex, ngrok, ffmpeg…) are found.
#[cfg(unix)]
fn fix_unix_path() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let login_path = std::process::Command::new(&shell)
        .args(["-ilc", "printf '__PATH__%s__PATH__' \"$PATH\""])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split("__PATH__").nth(1).map(str::to_string))
        .unwrap_or_default();
    let mut path = format!("{login_path}:{}", std::env::var("PATH").unwrap_or_default());
    let home = home_dir();
    let mut candidates: Vec<String> = vec![
        format!("{home}/.local/bin"), format!("{home}/.claude/local"), format!("{home}/.bun/bin"),
        format!("{home}/.npm-global/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(),
    ];
    if let Ok(versions) = std::fs::read_dir(format!("{home}/.nvm/versions/node")) {
        candidates.extend(versions.filter_map(Result::ok).map(|v| format!("{}/bin", v.path().display())));
    }
    for bin in ["claude", "codex", "ngrok", "cloudflared"] {
        let found = path.split(':').any(|d| std::path::Path::new(d).join(bin).exists());
        if let (false, Some(dir)) = (found, candidates.iter().find(|d| std::path::Path::new(d).join(bin).exists())) {
            path = format!("{dir}:{path}");
        }
    }
    // SAFETY: runs at startup before any other thread exists.
    unsafe { std::env::set_var("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{path}")) };
}

fn main() {
    #[cfg(unix)]
    fix_unix_path();

    let home = home_dir();
    assert!(!home.is_empty(), "HOME/USERPROFILE not set");
    let dir = PathBuf::from(home).join(".agentboard");
    std::fs::create_dir_all(&dir).expect("create data dir");
    let conn = Connection::open(dir.join("board.db")).expect("open db");
    conn.execute_batch(SCHEMA).expect("init schema");
    for alter in [
        "ALTER TABLE cards ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE projects ADD COLUMN wiki TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE projects ADD COLUMN wiki_status TEXT NOT NULL DEFAULT 'none'",
        "ALTER TABLE projects ADD COLUMN wiki_files INTEGER NOT NULL DEFAULT 0",
    ] {
        if let Err(e) = conn.execute(alter, []) {
            assert!(e.to_string().contains("duplicate column"), "migration failed: {e}");
        }
    }
    // An indexing run interrupted by a restart would stay "indexing" forever.
    conn.execute("UPDATE projects SET wiki_status='none' WHERE wiki_status='indexing'", []).unwrap();
    let token = setting(&conn, "token", || random_hex(16)).unwrap();
    let ntfy_topic = setting(&conn, "ntfy_topic", || format!("agentboard-{}", random_hex(8))).unwrap();
    let ntfy_server = std::env::var("NTFY_SERVER").unwrap_or_else(|_| "https://ntfy.sh".into());
    // Cards left "running" by a previous crash would block new runs in the UI.
    conn.execute("UPDATE cards SET status='idle' WHERE status='running'", []).unwrap();

    let app = Arc::new(App {
        db: Mutex::new(conn),
        tx: broadcast::channel(256).0,
        token: token.clone(),
        dir,
        running: Mutex::new(HashMap::new()),
        http: reqwest::Client::new(),
        ntfy_server,
        ntfy_topic: ntfy_topic.clone(),
        public_url: Mutex::new(String::new()),
        tunnel_restart: tokio::sync::Notify::new(),
    });

    let rt = tokio::runtime::Runtime::new().unwrap();
    let port: u16 = std::env::var("AGENTBOARD_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7777);
    let headless = std::env::args().any(|a| a == "--headless");
    match rt.block_on(tokio::net::TcpListener::bind(("0.0.0.0", port))) {
        Ok(listener) => {
            let tunnel_app = app.clone();
            rt.spawn(async move { tunnel::start(tunnel_app, port) });
            let router = router(app);
            rt.spawn(async move { axum::serve(listener, router).await.unwrap() });
        }
        // Another instance already serves this port (same DB/token): just open a window on it.
        Err(e) if !headless => eprintln!("porta {port} em uso ({e}); abrindo janela na instância existente"),
        Err(e) => panic!("bind port {port}: {e}"),
    }

    println!("AgentBoard  http://127.0.0.1:{port}/?token={token}");
    println!("Celular     http://{}:{port}/?token={token}", lan_ip());
    println!("Push (ntfy) topic: {ntfy_topic}");

    if headless {
        rt.block_on(std::future::pending::<()>());
    }
    window(&format!("http://127.0.0.1:{port}/?token={token}"));
}

fn window(url: &str) {
    use tao::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoop},
        window::WindowBuilder,
    };
    let event_loop = EventLoop::new();
    #[cfg(target_os = "macos")]
    native_menu();
    let win = WindowBuilder::new()
        .with_title("AgentBoard")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .build(&event_loop)
        .expect("create window");
    #[cfg(not(target_os = "linux"))]
    let _webview = wry::WebViewBuilder::new().with_url(url).build(&win).expect("create webview");
    #[cfg(target_os = "linux")]
    let _webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = win.default_vbox().expect("gtk container");
        wry::WebViewBuilder::new().with_url(url).build_gtk(vbox).expect("create webview")
    };
    event_loop.run(move |ev, _, flow| {
        *flow = ControlFlow::Wait;
        if let Event::WindowEvent { event: WindowEvent::CloseRequested, .. } = ev {
            *flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "macos")]
/// WKWebView needs a real Edit menu for Cmd+C/V/X/A/Z, and an app menu for Cmd+Q/H.
fn native_menu() {
    use muda::{Menu, PredefinedMenuItem as P, Submenu};
    let menu = Menu::new();
    let app = Submenu::new("AgentBoard", true);
    let edit = Submenu::new("Editar", true);
    let window = Submenu::new("Janela", true);
    let _ = app.append_items(&[&P::about(None, None), &P::separator(), &P::hide(None), &P::hide_others(None), &P::separator(), &P::quit(None)]);
    let _ = edit.append_items(&[&P::undo(None), &P::redo(None), &P::separator(), &P::cut(None), &P::copy(None), &P::paste(None), &P::select_all(None)]);
    let _ = window.append_items(&[&P::minimize(None), &P::close_window(None)]);
    let _ = menu.append_items(&[&app, &edit, &window]);
    menu.init_for_nsapp();
    std::mem::forget(menu); // the menu must outlive the event loop
}
