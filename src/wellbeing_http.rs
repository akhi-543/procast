//! The Screen Time dashboard: a small HTTP server on 127.0.0.1:8737 (port of `tracker.py`'s
//! `Handler`). One request per connection, headers ≤ 8 KiB, body ≤ 1 KiB. Every request must
//! carry our own `Host` (DNS rebinding); the one POST must also carry our own `Origin`.
use crate::win::Msg;
use crate::{config, focus, wellbeing};
use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PORT: u16 = 8737;
const HOSTS: [&str; 2] = ["127.0.0.1:8737", "localhost:8737"];
const ORIGINS: [&str; 2] = ["http://127.0.0.1:8737", "http://localhost:8737"];
const API: [&str; 3] = ["/api/stats", "/api/days", "/api/export.csv"];
const DASHBOARD: &str = include_str!("../assets/screen-time/dashboard.html");
const MAX_HEAD: u64 = 8 * 1024;
const MAX_BODY: usize = 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub host: String,
    pub origin: String,
    /// Empty when the request sent none, or more than `MAX_BODY` (then it is never read).
    pub body: Vec<u8>,
}

pub struct Response {
    pub code: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
    pub filename: Option<String>,
}

fn reply(code: u16, ctype: &'static str, body: Vec<u8>) -> Response {
    Response { code, ctype, body, filename: None }
}

fn empty(code: u16) -> Response {
    reply(code, "application/json", vec![])
}

/// 500 with `{"error": …}`, which the dashboard shows.
fn fail(error: impl ToString) -> Response {
    reply(500, "application/json", json!({ "error": error.to_string() }).to_string().into_bytes())
}

fn host_ok(req: &Request) -> bool {
    HOSTS.contains(&req.host.as_str())
}

/// Answers one request. `db` and `cfg_path` are the database and config.toml.
pub fn route(req: &Request, db: &Path, cfg_path: &Path) -> Response {
    if !host_ok(req) {
        return empty(403);
    }
    match req.method.as_str() {
        "GET" if req.path == "/" => reply(200, "text/html; charset=utf-8", DASHBOARD.as_bytes().to_vec()),
        "GET" if API.contains(&req.path.as_str()) => match span(&req.query) {
            Some((from, to)) => api(&req.path, from, to, db, cfg_path).unwrap_or_else(fail),
            None => empty(400),
        },
        "GET" => empty(404),
        "POST" => post(req, cfg_path),
        _ => empty(405),
    }
}

/// `from` and `to` (first of each, `YYYY-MM-DD`) as day numbers, 0–400 days apart.
fn span(query: &str) -> Option<(i64, i64)> {
    let get = |key: &str| query.split('&').find_map(|p| p.strip_prefix(key)?.strip_prefix('=')).and_then(wellbeing::parse_iso);
    let (from, to) = (get("from")?, get("to")?);
    (0..=400).contains(&(to - from)).then_some((from, to))
}

/// The stats, per-day totals or CSV of `from..=to`; the span is already checked.
fn api(path: &str, from: i64, to: i64, db: &Path, cfg_path: &Path) -> Result<Response, String> {
    let cfg = config::parse(&focus::text_or_empty(std::fs::read_to_string(cfg_path))?)?;
    let cats = focus::categories(&cfg.focus_category);
    let rules = wellbeing::rules(&cats);
    let sql = |e: rusqlite::Error| e.to_string();
    let con = wellbeing::open(db).map_err(sql)?;
    let mut body = match path {
        "/api/export.csv" => {
            let csv = wellbeing::export_csv(&con, from, to, &rules).map_err(sql)?;
            let name = format!("screen-time-{}-to-{}.csv", wellbeing::iso(from), wellbeing::iso(to));
            return Ok(Response { filename: Some(name), ..reply(200, "text/csv; charset=utf-8", csv) });
        }
        "/api/days" => wellbeing::days(&con, from, to, &rules).map_err(sql)?,
        _ => {
            let mut stats = wellbeing::stats(&con, from, to, wellbeing::prior(from, to), &rules).map_err(sql)?;
            stats["options"] = cats.iter().map(|c| c.name.as_str()).collect();
            stats
        }
    };
    body["kinds"] = wellbeing::kinds(&cfg.screen_time);
    Ok(reply(200, "application/json", body.to_string().into_bytes()))
}

/// `POST /api/category` `{"name", "category"}`: move an app or site to another category.
fn post(req: &Request, cfg_path: &Path) -> Response {
    if req.path != "/api/category" || !ORIGINS.contains(&req.origin.as_str()) {
        return empty(403);
    }
    let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
    let (Some(name), Some(category)) = (body["name"].as_str(), body["category"].as_str()) else {
        return empty(400);
    };
    let mut rejected = false; // move_name refused the input (400), as opposed to a config that can't be read or written (500)
    let done = config::edit_file(cfg_path, |text| {
        let cats = focus::categories(&config::parse(text)?.focus_category);
        wellbeing::move_name(text, &cats, name, category).inspect_err(|_| rejected = true)
    });
    match done {
        Ok(_) => empty(204), // procast's config poll picks the change up
        Err(_) if rejected => empty(400),
        Err(e) => fail(e),
    }
}

/// One line without its ending; None at the end of input or when the line is cut off.
fn head_line(r: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    let n = r.read_line(&mut line).ok()?;
    (n > 0 && line.ends_with('\n')).then(|| line.trim_end_matches(['\r', '\n']).to_string())
}

/// Reads one request. None if the head is malformed or over 8 KiB. A body over 1 KiB is not read
/// (the request comes back with none, which `route` answers 400).
pub fn parse_request(r: &mut impl BufRead) -> Option<Request> {
    let mut head = (&mut *r).take(MAX_HEAD + 1);
    let first = head_line(&mut head)?;
    let mut parts = first.split_ascii_whitespace();
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut req = Request { method: method.into(), path: path.into(), query: query.into(), ..Default::default() };
    let (mut host, mut origin, mut length) = (None, None, None);
    loop {
        let line = head_line(&mut head)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        let slot = match name.to_ascii_lowercase().as_str() {
            "host" => &mut host,
            "origin" => &mut origin,
            "content-length" => &mut length,
            _ => continue,
        };
        // Two of these could be read two ways by two parties: refuse.
        if slot.replace(value.trim().to_string()).is_some() {
            return None;
        }
    }
    if head.limit() == 0 {
        return None; // 8 KiB and more
    }
    (req.host, req.origin) = (host.unwrap_or_default(), origin.unwrap_or_default());
    let length = length.and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
    if (1..=MAX_BODY).contains(&length) {
        r.take(length as u64).read_to_end(&mut req.body).ok()?;
    }
    Some(req)
}

fn write_response(w: &mut impl Write, res: &Response) -> io::Result<()> {
    let reason = match res.code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nConnection: close\r\n",
        res.code,
        res.ctype,
        res.body.len()
    );
    if let Some(name) = &res.filename {
        head += &format!("Content-Disposition: attachment; filename=\"{name}\"\r\n");
    }
    w.write_all(head.as_bytes())?;
    w.write_all(b"\r\n")?;
    w.write_all(&res.body)
}

fn handle(mut stream: TcpStream, flush: &(dyn Fn() + Send + Sync), db: &Path, cfg_path: &Path) {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let res = match parse_request(&mut BufReader::new(&stream)) {
        None => empty(400),
        Some(req) => {
            // What the sampler holds in memory goes to the database first, so the numbers are current.
            if req.method == "GET" && API.contains(&req.path.as_str()) && host_ok(&req) {
                flush();
            }
            route(&req, db, cfg_path)
        }
    };
    let _ = write_response(&mut stream, &res);
    let _ = stream.shutdown(Shutdown::Both);
}

/// One thread per connection (each ends within the timeouts, or when the request is answered).
fn serve_on(listener: TcpListener, flush: Arc<dyn Fn() + Send + Sync>, db: PathBuf, cfg_path: PathBuf) {
    for stream in listener.incoming().flatten() {
        let (flush, db, cfg_path) = (flush.clone(), db.clone(), cfg_path.clone());
        std::thread::spawn(move || handle(stream, &*flush, &db, &cfg_path));
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the dashboard in the background; a no-op while it is running. `flush` writes the
/// sampler's pending counters (called before each stats request, off the UI thread). If the port
/// is taken, says so once and Screen Time carries on without a dashboard.
pub fn serve(tx: UnboundedSender<Msg>, flush: impl Fn() + Send + Sync + 'static) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            let flush: Arc<dyn Fn() + Send + Sync> = Arc::new(flush);
            std::thread::spawn(move || serve_on(listener, flush, wellbeing::db_path(), config::path()));
        }
        Err(_) => {
            RUNNING.store(false, Ordering::SeqCst); // a later enable tries again
            let _ = tx.unbounded_send(Msg::error("Port 8737 is in use: is micro bot still running?"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    const CONFIG: &str = "[screen_time]\nproductive = [\"Development\"]\ndistracting = [\"Social\"]\n";

    /// A fresh folder with a database holding one day and a config.toml.
    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("procast-http-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (db, cfg) = (dir.join("screentime.db"), dir.join("config.toml"));
        let con = wellbeing::open(&db).unwrap();
        con.execute("INSERT INTO usage VALUES('2026-09-30', 9, 'app', 'code.exe', 600.0)", []).unwrap();
        std::fs::write(&cfg, CONFIG).unwrap();
        (dir, db, cfg)
    }

    fn get(path: &str, query: &str) -> Request {
        Request { method: "GET".into(), path: path.into(), query: query.into(), host: "127.0.0.1:8737".into(), ..Default::default() }
    }

    fn post_category(origin: &str, body: &str) -> Request {
        Request { method: "POST".into(), path: "/api/category".into(), host: "localhost:8737".into(), origin: origin.into(), body: body.as_bytes().to_vec(), ..Default::default() }
    }

    const DAY: &str = "from=2026-09-30&to=2026-09-30";

    fn json_of(res: &Response) -> Value {
        serde_json::from_slice(&res.body).unwrap()
    }

    #[test]
    fn host_guard() {
        let (dir, db, cfg) = setup("host");
        for path in ["/", "/api/stats"] {
            let mut req = get(path, DAY);
            assert_eq!(route(&req, &db, &cfg).code, 200, "{path}");
            req.host = "localhost:8737".into();
            assert_eq!(route(&req, &db, &cfg).code, 200, "{path}");
            for host in ["evil.com:8737", "evil.com", "127.0.0.1", "127.0.0.1:8738", ""] {
                req.host = host.into();
                assert_eq!(route(&req, &db, &cfg).code, 403, "{path} {host:?}");
            }
        }
        let mut req = post_category("http://127.0.0.1:8737", r#"{"name":"x.com","category":"Development"}"#);
        req.host = "evil.com:8737".into();
        assert_eq!(route(&req, &db, &cfg).code, 403);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn origin_guard() {
        let (dir, db, cfg) = setup("origin");
        let body = r#"{"name":"x.com","category":"Development"}"#;
        for origin in ["https://evil.com", "http://evil.com:8737", "http://127.0.0.1:8738", "chrome-extension://abc", ""] {
            assert_eq!(route(&post_category(origin, body), &db, &cfg).code, 403, "{origin:?}");
        }
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), CONFIG, "refused requests change nothing");
        let mut tab = post_category("http://127.0.0.1:8737", body);
        tab.path = "/tab".into();
        assert_eq!(route(&tab, &db, &cfg).code, 403);

        let res = route(&post_category("http://127.0.0.1:8737", body), &db, &cfg);
        assert_eq!(res.code, 204);
        let cats = focus::categories(&config::parse(&std::fs::read_to_string(&cfg).unwrap()).unwrap().focus_category);
        assert!(focus::category(&cats, "Development").unwrap().sites.contains(&"x.com".to_string()));
        assert!(std::fs::read_to_string(&cfg).unwrap().contains("[screen_time]"), "the rest of config.toml stays");
        assert_eq!(route(&post_category("http://localhost:8737", r#"{"name":"code.exe","category":"Other"}"#), &db, &cfg).code, 204);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn post_rejects_bad_bodies() {
        let (dir, db, cfg) = setup("badpost");
        let origin = "http://127.0.0.1:8737";
        for body in ["", "x", "[]", r#"{"name":"x.com"}"#, r#"{"name":1,"category":"Other"}"#, r#"{"name":"x.com","category":"Nope"}"#, r#"{"name":"a b","category":"Other"}"#, r#"{"name":"","category":"Other"}"#] {
            assert_eq!(route(&post_category(origin, body), &db, &cfg).code, 400, "{body:?}");
        }
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), CONFIG);
        // a config.toml that can't be read is a server error, not a bad request
        let res = route(&post_category(origin, r#"{"name":"x.com","category":"Other"}"#), &db, &dir);
        assert_eq!(res.code, 500);
        assert!(json_of(&res)["error"].as_str().unwrap().contains("config.toml"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn span_and_dates() {
        let (dir, db, cfg) = setup("span");
        let code = |q: &str| route(&get("/api/stats", q), &db, &cfg).code;
        assert_eq!(code("from=2026-09-30&to=2026-09-29"), 400);
        assert_eq!(code("from=2025-08-25&to=2026-09-30"), 400); // 401 days
        assert_eq!(code("from=2026-02-30&to=2026-03-01"), 400);
        assert_eq!(code("from=x&to=2026-09-30"), 400);
        assert_eq!(code("from=2026-09-30"), 400);
        assert_eq!(code("to=2026-09-30"), 400);
        assert_eq!(code(""), 400);
        assert_eq!(code("from=2026-9-30&to=2026-09-30"), 400);
        assert_eq!(code("from=2026-09-30&to=2026-09-30&from=bad"), 200, "the first one counts");
        assert_eq!(code("from=bad&from=2026-09-30&to=2026-09-30"), 400);
        assert_eq!(code("from=2026-09-30&to=2026-09-30"), 200); // 0 days
        assert_eq!(code("from=2025-08-26&to=2026-09-30"), 200); // 400 days
        for path in ["/api/days", "/api/export.csv"] {
            assert_eq!(route(&get(path, "from=2025-08-25&to=2026-09-30"), &db, &cfg).code, 400, "{path}");
            assert_eq!(route(&get(path, "from=2026-09-30&to=2026-09-29"), &db, &cfg).code, 400, "{path}");
            assert_eq!(route(&get(path, "to=2026-09-30"), &db, &cfg).code, 400, "{path}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stats_body_has_options_and_kinds() {
        let (dir, db, cfg) = setup("stats");
        let res = route(&get("/api/stats", DAY), &db, &cfg);
        assert_eq!((res.code, res.ctype), (200, "application/json"));
        let body = json_of(&res);
        let names: Vec<_> = focus::categories(&[]).iter().map(|c| c.name.clone()).collect();
        assert_eq!(body["options"], json!(names));
        assert_eq!(body["kinds"], json!({"Development": "productive", "Social": "distracting"}));
        assert_eq!(body["total"], 600);
        assert_eq!(body["previous"], 0);

        let days = json_of(&route(&get("/api/days", "from=2026-09-29&to=2026-09-30"), &db, &cfg));
        assert_eq!(days["days"], json!({"2026-09-29": {}, "2026-09-30": {"Development": 600}}));
        assert_eq!(days["kinds"]["Social"], "distracting");

        // a config that does not parse is shown, not hidden
        std::fs::write(&cfg, "[screen_time\n").unwrap();
        let res = route(&get("/api/stats", DAY), &db, &cfg);
        assert_eq!(res.code, 500);
        assert!(json_of(&res)["error"].is_string());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn csv_download() {
        let (dir, db, cfg) = setup("csv");
        let res = route(&get("/api/export.csv", DAY), &db, &cfg);
        assert_eq!(res.code, 200);
        assert_eq!(res.ctype, "text/csv; charset=utf-8");
        assert_eq!(res.filename.as_deref(), Some("screen-time-2026-09-30-to-2026-09-30.csv"));
        let text = String::from_utf8(res.body.clone()).unwrap();
        assert!(text.starts_with("\u{feff}day,hour,kind,name,category,seconds\r\n2026-09-30,9,app,code.exe,Development,600"));
        let mut out = vec![];
        write_response(&mut out, &res).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(out.contains("Content-Disposition: attachment; filename=\"screen-time-2026-09-30-to-2026-09-30.csv\"\r\n"));
        assert!(out.contains("Cache-Control: no-store\r\n") && out.contains("Connection: close\r\n"));
        assert!(out.contains(&format!("Content-Length: {}\r\n", res.body.len())));
        assert!(out.ends_with(&format!("\r\n\r\n{text}")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unknown_path_404() {
        let (dir, db, cfg) = setup("404");
        for path in ["/tab", "/api", "/api/stats/", "/index.html", "/api/category", "//"] {
            assert_eq!(route(&get(path, DAY), &db, &cfg).code, 404, "{path}");
        }
        let mut req = get("/", "");
        req.method = "DELETE".into();
        assert_eq!(route(&req, &db, &cfg).code, 405);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn parse(raw: &str) -> Option<Request> {
        parse_request(&mut raw.as_bytes())
    }

    #[test]
    fn parse_reads_head_and_body() {
        let req = parse("POST /api/category?a=b HTTP/1.1\r\nhOsT: 127.0.0.1:8737\r\nOrigin: http://127.0.0.1:8737\r\nContent-Length: 5\r\n\r\nhelloEXTRA").unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str(), req.query.as_str()), ("POST", "/api/category", "a=b"));
        assert_eq!((req.host.as_str(), req.origin.as_str(), req.body.as_slice()), ("127.0.0.1:8737", "http://127.0.0.1:8737", &b"hello"[..]));
        let req = parse("GET / HTTP/1.1\nHost: localhost:8737\n\n").unwrap();
        assert_eq!((req.path.as_str(), req.query.as_str(), req.host.as_str()), ("/", "", "localhost:8737"));
        assert!(req.body.is_empty() && req.origin.is_empty());
        // a body that ends early is kept as far as it went; route's JSON check refuses it
        assert_eq!(parse("POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\nabc").unwrap().body, b"abc");
    }

    #[test]
    fn parse_rejects_malformed_heads() {
        for raw in [
            "",
            "GET /\r\n\r\n",
            "GET / HTTP/1.1 extra\r\n\r\n",
            "GET / SPDY/3\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: a\r\n", // no blank line
            "GET / HTTP/1.1\r\nno colon\r\n\r\n",
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:8737\r\nHost: evil.com\r\n\r\n",
            "POST / HTTP/1.1\r\nOrigin: a\r\nOrigin: b\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            "GET / HTTP/1.1",
        ] {
            assert!(parse(raw).is_none(), "{raw:?}");
        }
        let long = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(8192));
        assert!(parse(&long).is_none(), "head over 8 KiB");
        let long = format!("GET /{} HTTP/1.1", "a".repeat(9000)); // a line that never ends
        assert!(parse(&long).is_none());
        let fits = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(8000));
        assert!(parse(&fits).is_some());
    }

    #[test]
    fn parse_rejects_big_body() {
        let (dir, db, cfg) = setup("big");
        let body = "x".repeat(5000);
        let raw = format!("POST /api/category HTTP/1.1\r\nHost: 127.0.0.1:8737\r\nOrigin: http://127.0.0.1:8737\r\nContent-Length: 5000\r\n\r\n{body}");
        let mut reader = raw.as_bytes();
        let req = parse_request(&mut reader).unwrap();
        assert!(req.body.is_empty());
        assert_eq!(reader.len(), 5000, "the body is left unread");
        assert_eq!(route(&req, &db, &cfg).code, 400);
        // exactly 1 KiB is fine; 1025 is not
        let ok = format!("POST / HTTP/1.1\r\nContent-Length: 1024\r\n\r\n{}", "x".repeat(1024));
        assert_eq!(parse(&ok).unwrap().body.len(), 1024);
        let big = format!("POST / HTTP/1.1\r\nContent-Length: 1025\r\n\r\n{}", "x".repeat(1025));
        assert!(parse(&big).unwrap().body.is_empty());
        for length in ["-1", "abc", "", "99999999999999999999999"] {
            assert!(parse(&format!("POST / HTTP/1.1\r\nContent-Length: {length}\r\n\r\nabc")).unwrap().body.is_empty(), "{length:?}");
        }
        // the route itself refuses an oversize body too
        let mut req = post_category("http://127.0.0.1:8737", &format!(r#"{{"name":"{}","category":"Other"}}"#, "a".repeat(2000)));
        req.body.truncate(2000);
        assert_eq!(route(&req, &db, &cfg).code, 400);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The whole thing over a real socket, on a free port (never 8737).
    #[test]
    fn socket_round_trip_flushes_first() {
        let (dir, db, cfg) = setup("socket");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let flushes = Arc::new(AtomicUsize::new(0));
        let counter = flushes.clone();
        let flush: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let (d, c) = (db.clone(), cfg.clone());
        std::thread::spawn(move || serve_on(listener, flush, d, c));
        let ask = |raw: &str| {
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let out = ask(&format!("GET /api/stats?{DAY} HTTP/1.1\r\nHost: 127.0.0.1:8737\r\n\r\n"));
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n") && out.contains("\"options\""), "{out}");
        assert_eq!(flushes.load(Ordering::SeqCst), 1);
        let out = ask(&format!("GET /api/stats?{DAY} HTTP/1.1\r\nHost: evil.com\r\n\r\n"));
        assert!(out.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{out}");
        assert!(ask("GET / HTTP/1.1\r\nHost: localhost:8737\r\n\r\n").contains("<html"));
        assert_eq!(flushes.load(Ordering::SeqCst), 1, "only API requests with our Host flush");
        assert!(ask("nonsense\r\n\r\n").starts_with("HTTP/1.1 400 Bad Request\r\n"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
