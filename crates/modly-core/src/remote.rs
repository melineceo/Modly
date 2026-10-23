//! Пульт с телефона: маленький веб-сервер Modly в домашней сети.
//!
//! Телефон открывает `http://<IP компьютера>:<порт>/#k=<ключ>` (это и есть QR-код в настройках) и
//! видит ту же панель мода, что и Modly: переключатели, ползунки, кнопки.
//!
//! Защита:
//! - сервер включается только тумблером в настройках и слушает только при включённом тумблере;
//! - запросы принимаются только из своей сети (частные адреса IPv4/IPv6 и этот компьютер);
//! - на каждый запрос к API нужен ключ (`Authorization: Bearer`): 192 случайных бита, сравнение без
//!   утечки по времени; после 20 неверных ключей с адреса за минуту — пауза;
//! - ключ в QR-коде передаётся после `#` — такая часть адреса не уходит на сервер и не попадает в журналы;
//! - `Host` должен быть IP-адресом (защита от DNS rebinding), ответы — с CSP без чужих скриптов;
//! - «Сбросить доступ» выдаёт новый ключ — все телефоны отвязываются.
//!
//! Сам интерфейс телефона — `crates/modly-app/remote/` (HTML, CSS, JS без внешних ресурсов).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Порт по умолчанию (если занят — любой свободный).
pub const DEFAULT_PORT: u16 = 47777;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;

/// Что телефон просит сделать с панелью мода.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Command {
    /// Переключатель или текст: значение как есть.
    Set { id: String, value: Value },
    /// Ползунок или число.
    Number { id: String, value: f64 },
    /// Выбор варианта (один или несколько).
    Pick { id: String, option: String },
    /// Кнопка.
    Invoke { id: String },
    /// Страница панели.
    Page { index: i32 },
}

/// Связь сервера с приложением: снимок панели (JSON) и команды. Вызывается из потоков сервера.
pub trait Panel: Send + Sync + 'static {
    /// Снимок панели для телефона (в JSON). Может ждать поток интерфейса.
    fn snapshot(&self) -> tokio::sync::oneshot::Receiver<Value>;
    fn command(&self, cmd: Command);
}

/// Файлы страницы пульта.
pub struct Assets {
    pub html: &'static str,
    pub css: &'static str,
    pub js: &'static str,
    pub icon: &'static str,
}

/// Сохранённый ключ доступа (в папке данных Modly).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Saved {
    key: String,
    port: u16,
}

fn saved_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("remote.json")
}

/// Ключ доступа: сохранённый или новый.
pub fn load_or_create_key(reset: bool) -> Result<(String, u16)> {
    let mut saved: Saved = std::fs::read(saved_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if reset || saved.key.len() != 32 || !saved.key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        let mut raw = [0u8; 24];
        #[cfg(windows)]
        crate::win::random_bytes(&mut raw)?;
        saved.key = b64url(&raw);
    }
    if saved.port == 0 {
        saved.port = DEFAULT_PORT;
    }
    crate::write_atomic(&saved_file(), &serde_json::to_vec_pretty(&saved)?)?;
    Ok((saved.key, saved.port))
}

fn remember_port(port: u16) {
    if let Ok(b) = std::fs::read(saved_file()) {
        if let Ok(mut s) = serde_json::from_slice::<Saved>(&b) {
            if s.port != port {
                s.port = port;
                let _ = crate::write_atomic(&saved_file(), &serde_json::to_vec_pretty(&s).unwrap_or_default());
            }
        }
    }
}

/// IPv4-адрес этого компьютера в домашней сети (по маршруту по умолчанию; пакеты не отправляются).
pub fn lan_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?; // TEST-NET-1: маршрут есть, адрес — ничей
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// Адрес из своей сети: частные диапазоны, link-local и этот компьютер.
pub fn is_local_client(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_client(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// Работающий сервер пульта. Остановка — `stop()` или когда объект уничтожается.
pub struct Server {
    pub port: u16,
    shared: Arc<Shared>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Server {
    /// Новый ключ («Сбросить доступ»): старый перестаёт действовать сразу, порт остаётся тем же.
    pub fn set_key(&self, key: String) {
        *self.shared.key.lock().unwrap() = key;
        self.shared.fails.lock().unwrap().clear();
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Shared {
    key: Mutex<String>,
    panel: Arc<dyn Panel>,
    assets: Assets,
    fails: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

/// Запустить сервер на `port` (или свободном, если он занят). Слушает все адреса — проверка «своя
/// сеть» идёт на каждом соединении.
pub fn start(rt: &tokio::runtime::Handle, key: String, port: u16, panel: Arc<dyn Panel>, assets: Assets) -> Result<Server> {
    let std_listener = std::net::TcpListener::bind(("0.0.0.0", port))
        .or_else(|_| std::net::TcpListener::bind(("0.0.0.0", 0)))
        .context("не удалось открыть порт для пульта")?;
    std_listener.set_nonblocking(true)?;
    let port = std_listener.local_addr()?.port();
    remember_port(port);
    let shared = Arc::new(Shared { key: Mutex::new(key), panel, assets, fails: Mutex::new(HashMap::new()) });
    let keep = shared.clone();
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let _guard = rt.enter();
    let listener = TcpListener::from_std(std_listener)?;
    rt.spawn(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                acc = listener.accept() => {
                    let Ok((stream, addr)) = acc else { continue };
                    if !is_local_client(addr.ip()) {
                        continue; // чужая сеть — молча закрываем
                    }
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(15), handle(stream, addr, shared)).await;
                    });
                }
            }
        }
        log::info!("remote: stopped");
    });
    log::info!("remote: listening on port {port}");
    Ok(Server { port, shared: keep, stop: Some(tx) })
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Some(Request { method, path, headers, body })
}

async fn respond(stream: &mut TcpStream, status: u16, ctype: &str, body: &[u8], extra: &str) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\
         {extra}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
}

/// Host — только IP-адрес (с портом) или localhost: страницы с чужих доменов (DNS rebinding) не пройдут.
fn host_ok(host: &str) -> bool {
    if host.parse::<IpAddr>().is_ok() {
        return true; // IPv6 без порта
    }
    let h = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host),
    };
    h.eq_ignore_ascii_case("localhost") || h.parse::<IpAddr>().is_ok()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn handle(mut stream: TcpStream, addr: SocketAddr, s: Arc<Shared>) {
    let Some(req) = read_request(&mut stream).await else { return };
    if !req.headers.get("host").is_some_and(|h| host_ok(h)) {
        respond(&mut stream, 400, "text/plain", b"bad host", "").await;
        return;
    }
    let path = req.path.split('?').next().unwrap_or("/").to_owned();
    // страница пульта — без ключа (ключ — в #части адреса, её читает сам скрипт)
    match (req.method.as_str(), path.as_str()) {
        ("GET", "/") => return respond(&mut stream, 200, "text/html; charset=utf-8", s.assets.html.as_bytes(), "").await,
        ("GET", "/remote.css") => return respond(&mut stream, 200, "text/css; charset=utf-8", s.assets.css.as_bytes(), "").await,
        ("GET", "/remote.js") => return respond(&mut stream, 200, "text/javascript; charset=utf-8", s.assets.js.as_bytes(), "").await,
        ("GET", "/icon.svg") => return respond(&mut stream, 200, "image/svg+xml", s.assets.icon.as_bytes(), "").await,
        ("GET", "/favicon.ico") => return respond(&mut stream, 404, "text/plain", b"", "").await,
        _ => {}
    }
    if !path.starts_with("/api/") {
        return respond(&mut stream, 404, "text/plain", b"not found", "").await;
    }

    // ключ
    let ip = addr.ip();
    let blocked = {
        let fails = s.fails.lock().unwrap();
        fails.get(&ip).is_some_and(|(since, n)| since.elapsed() < Duration::from_secs(60) && *n >= 20)
    };
    if blocked {
        return respond(&mut stream, 429, "application/json", br#"{"error":"too many attempts"}"#, "Retry-After: 60\r\n").await;
    }
    let sent = req.headers.get("authorization").and_then(|h| h.strip_prefix("Bearer ")).unwrap_or("");
    let good = ct_eq(sent.as_bytes(), s.key.lock().unwrap().as_bytes());
    if !good {
        {
            let mut fails = s.fails.lock().unwrap();
            let e = fails.entry(ip).or_insert((Instant::now(), 0));
            if e.0.elapsed() > Duration::from_secs(60) {
                *e = (Instant::now(), 0);
            }
            e.1 += 1;
        }
        return respond(&mut stream, 401, "application/json", br#"{"error":"bad key"}"#, "").await;
    }

    match (req.method.as_str(), path.as_str()) {
        ("GET", "/api/state") => {
            let snap = s.panel.snapshot().await.unwrap_or(Value::Null);
            let body = serde_json::to_vec(&snap).unwrap_or_default();
            respond(&mut stream, 200, "application/json", &body, "").await;
        }
        ("POST", "/api/command") => {
            // только JSON: обычная HTML-форма с чужой страницы такой запрос не отправит
            if !req.headers.get("content-type").is_some_and(|t| t.starts_with("application/json")) {
                return respond(&mut stream, 400, "application/json", br#"{"error":"json only"}"#, "").await;
            }
            match serde_json::from_slice::<Command>(&req.body) {
                Ok(cmd) if command_ok(&cmd) => {
                    s.panel.command(cmd);
                    respond(&mut stream, 204, "application/json", b"", "").await;
                }
                _ => respond(&mut stream, 400, "application/json", br#"{"error":"bad command"}"#, "").await,
            }
        }
        _ => respond(&mut stream, 405, "application/json", br#"{"error":"method"}"#, "").await,
    }
}

/// Команда в разумных пределах: короткие id, текст не длиннее 2000 символов, конечные числа.
fn command_ok(cmd: &Command) -> bool {
    let id_ok = |id: &str| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control);
    match cmd {
        Command::Set { id, value } => {
            id_ok(id)
                && match value {
                    Value::Bool(_) => true,
                    Value::String(t) => t.chars().count() <= 2000,
                    _ => false,
                }
        }
        Command::Number { id, value } => id_ok(id) && value.is_finite(),
        Command::Pick { id, option } => id_ok(id) && option.len() <= 128,
        Command::Invoke { id } => id_ok(id),
        Command::Page { index } => (0..1000).contains(index),
    }
}

fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..=c.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_clients_and_ip_hosts() {
        for ip in ["192.168.1.5", "10.0.0.2", "172.16.4.1", "127.0.0.1", "169.254.1.1", "::1", "fe80::1", "fd00::5", "::ffff:192.168.0.3"] {
            assert!(is_local_client(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:1.1.1.1"] {
            assert!(!is_local_client(ip.parse().unwrap()), "{ip}");
        }
        for h in ["192.168.1.5:47777", "localhost:47777", "127.0.0.1", "[::1]:47777", "[fe80::1]"] {
            assert!(host_ok(h), "{h}");
        }
        for h in ["evil.com", "evil.com:47777", "192.168.1.5.nip.io:47777", ""] {
            assert!(!host_ok(h), "{h}");
        }
    }

    #[test]
    fn commands_are_parsed_strictly() {
        let c: Command = serde_json::from_str(r#"{"op":"set","id":"god","value":true}"#).unwrap();
        assert_eq!(c, Command::Set { id: "god".into(), value: Value::Bool(true) });
        assert!(command_ok(&c));
        let n: Command = serde_json::from_str(r#"{"op":"number","id":"speed","value":2.5}"#).unwrap();
        assert!(command_ok(&n));
        assert!(!command_ok(&Command::Set { id: "x".into(), value: serde_json::json!({"a": 1}) }));
        assert!(!command_ok(&Command::Invoke { id: String::new() }));
        assert!(!command_ok(&Command::Page { index: -1 }));
        assert!(serde_json::from_str::<Command>(r#"{"op":"shell","cmd":"rm"}"#).is_err());
    }

    struct FakePanel(Mutex<Vec<Command>>);
    impl Panel for FakePanel {
        fn snapshot(&self) -> tokio::sync::oneshot::Receiver<Value> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(serde_json::json!({"connected": true}));
            rx
        }
        fn command(&self, cmd: Command) {
            self.0.lock().unwrap().push(cmd);
        }
    }

    #[test]
    fn serves_page_and_requires_the_key() {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let panel = Arc::new(FakePanel(Mutex::new(Vec::new())));
        let key = "k".repeat(32);
        let assets = Assets { html: "<!doctype html>remote", css: "", js: "", icon: "" };
        let server = start(rt.handle(), key.clone(), 0, panel.clone(), assets).unwrap();
        let port = server.port;
        let ask = |raw: String| -> String {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(raw.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        };
        let page = ask(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("remote") && page.contains("Content-Security-Policy"));
        assert!(ask("GET / HTTP/1.1\r\nHost: evil.com\r\n\r\n".into()).starts_with("HTTP/1.1 400"));
        assert!(ask("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".into()).starts_with("HTTP/1.1 401"));
        let state = ask(format!("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\n\r\n"));
        assert!(state.starts_with("HTTP/1.1 200") && state.ends_with(r#"{"connected":true}"#));
        let body = r#"{"op":"invoke","id":"heal"}"#;
        let cmd = ask(format!(
            "POST /api/command HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert!(cmd.starts_with("HTTP/1.1 204"), "{cmd}");
        assert_eq!(panel.0.lock().unwrap().as_slice(), &[Command::Invoke { id: "heal".into() }]);
        // форма не JSON — отказ
        let form = ask(format!(
            "POST /api/command HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
        assert!(form.starts_with("HTTP/1.1 400"));
        // перебор ключа — пауза
        for _ in 0..20 {
            ask("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer nope\r\n\r\n".into());
        }
        assert!(ask(format!("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\n\r\n")).starts_with("HTTP/1.1 429"));
        // «Сбросить доступ»: старый ключ больше не подходит, новый — сразу
        let fresh = "n".repeat(32);
        server.set_key(fresh.clone());
        assert!(ask(format!("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\n\r\n")).starts_with("HTTP/1.1 401"));
        assert!(ask(format!("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {fresh}\r\n\r\n")).starts_with("HTTP/1.1 200"));
        drop(server);
    }
}
