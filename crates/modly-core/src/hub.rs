//! Хаб: сервер именованного канала, к которому подключаются моды.
//!
//! Каждое подключение — сессия. Первым сообщением мод присылает `hello`, Modly отвечает `welcome`,
//! дальше — `layout`/`state`/… от мода и `set`/`invoke` от Modly. Протокол — `modly-protocol`.

use modly_protocol::{ClientMsg, GameInfo, MAX_MESSAGE, ModInfo, PIPE_NAME, PROTOCOL_VERSION, ServerMsg, encode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

pub type SessionId = u64;

/// Сколько ждать `hello` после подключения.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Больше сессий одновременно не принимаем.
const MAX_SESSIONS: usize = 64;

#[derive(Clone, Debug)]
pub struct Hello {
    pub protocol: u32,
    pub mod_info: ModInfo,
    pub game: Option<GameInfo>,
    pub pid: Option<u32>,
    pub sdk: Option<String>,
}

#[derive(Debug)]
pub enum HubEvent {
    /// Канал открыт, моды могут подключаться.
    Listening,
    /// Канал открыть не удалось (например, его уже держит другой Modly).
    ListenFailed(String),
    Connected { id: SessionId, hello: Hello },
    Message { id: SessionId, msg: ClientMsg },
    /// Строка, которую не удалось разобрать: соединение не рвём, показываем в консоли разработчика.
    BadMessage { id: SessionId, error: String, line: String },
    Disconnected { id: SessionId },
    /// Сырой трафик для консоли разработчика (только если включён `set_trace`).
    Traffic { id: SessionId, outgoing: bool, line: String },
}

type EventSink = Arc<dyn Fn(HubEvent) + Send + Sync>;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

struct Inner {
    sessions: Mutex<HashMap<SessionId, mpsc::UnboundedSender<String>>>,
    next_id: AtomicU64,
    next_req: AtomicU64,
    lang: Mutex<String>,
    app_version: String,
    trace: AtomicBool,
    sink: EventSink,
}

impl Hub {
    /// Запустить хаб на канале по умолчанию ([`PIPE_NAME`]).
    pub fn start(
        rt: &tokio::runtime::Handle,
        app_version: &str,
        lang: &str,
        on_event: impl Fn(HubEvent) + Send + Sync + 'static,
    ) -> Hub {
        Self::start_on(rt, PIPE_NAME, app_version, lang, on_event)
    }

    pub fn start_on(
        rt: &tokio::runtime::Handle,
        pipe: &str,
        app_version: &str,
        lang: &str,
        on_event: impl Fn(HubEvent) + Send + Sync + 'static,
    ) -> Hub {
        let inner = Arc::new(Inner {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_req: AtomicU64::new(1),
            lang: Mutex::new(lang.to_owned()),
            app_version: app_version.to_owned(),
            trace: AtomicBool::new(false),
            sink: Arc::new(on_event),
        });
        rt.spawn(listen(pipe.to_owned(), inner.clone()));
        Hub { inner }
    }

    pub fn send(&self, id: SessionId, msg: &ServerMsg) -> bool {
        let line = encode(msg);
        let tx = self.inner.sessions.lock().unwrap().get(&id).cloned();
        match tx {
            Some(tx) => {
                self.inner.traffic(id, true, &line);
                tx.send(line).is_ok()
            }
            None => false,
        }
    }

    /// Пользователь изменил значение. Возвращает номер запроса.
    pub fn set(&self, id: SessionId, control: &str, value: Value) -> u64 {
        let req = self.inner.next_req.fetch_add(1, Ordering::Relaxed);
        self.send(id, &ServerMsg::Set { req, id: control.to_owned(), value });
        req
    }

    /// Пользователь нажал кнопку. Возвращает номер запроса.
    pub fn invoke(&self, id: SessionId, control: &str) -> u64 {
        let req = self.inner.next_req.fetch_add(1, Ordering::Relaxed);
        self.send(id, &ServerMsg::Invoke { req, id: control.to_owned() });
        req
    }

    /// Действие на карте мода (кнопка у метки или клик в пустое место). Возвращает номер запроса.
    pub fn map_action(&self, id: SessionId, map: &str, action: &str, marker: Option<&str>, x: f64, y: f64) -> u64 {
        let req = self.inner.next_req.fetch_add(1, Ordering::Relaxed);
        let msg = ServerMsg::MapAction {
            req,
            map: map.to_owned(),
            action: action.to_owned(),
            marker: marker.map(str::to_owned),
            x,
            y,
        };
        self.send(id, &msg);
        req
    }

    /// Сменить язык: новым сессиям — в `welcome`, текущим — сообщением `lang`.
    pub fn set_lang(&self, lang: &str) {
        *self.inner.lang.lock().unwrap() = lang.to_owned();
        let ids: Vec<SessionId> = self.inner.sessions.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.send(id, &ServerMsg::Lang { lang: lang.to_owned() });
        }
    }

    /// Присылать ли сырой трафик ([`HubEvent::Traffic`]) — для консоли разработчика.
    pub fn set_trace(&self, on: bool) {
        self.inner.trace.store(on, Ordering::Relaxed);
    }

    /// Закрыть сессию.
    pub fn disconnect(&self, id: SessionId) {
        self.inner.sessions.lock().unwrap().remove(&id);
    }

    pub fn session_count(&self) -> usize {
        self.inner.sessions.lock().unwrap().len()
    }
}

impl Inner {
    fn emit(&self, e: HubEvent) {
        (self.sink)(e);
    }

    fn traffic(&self, id: SessionId, outgoing: bool, line: &str) {
        if self.trace.load(Ordering::Relaxed) {
            self.emit(HubEvent::Traffic { id, outgoing, line: line.trim_end().to_owned() });
        }
    }
}

#[cfg(windows)]
async fn listen(pipe: String, inner: Arc<Inner>) {
    use tokio::net::windows::named_pipe::ServerOptions;

    let create = |first: bool| ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create(&pipe);
    let mut server = match create(true) {
        Ok(s) => s,
        Err(e) => {
            log::error!("hub: cannot open {pipe}: {e}");
            inner.emit(HubEvent::ListenFailed(e.to_string()));
            return;
        }
    };
    log::info!("hub: listening on {pipe}");
    inner.emit(HubEvent::Listening);
    loop {
        if let Err(e) = server.connect().await {
            log::warn!("hub: connect failed: {e}");
        } else {
            // следующий экземпляр канала — до того, как отдать этот, чтобы не было окна без сервера
            let next = loop {
                match create(false) {
                    Ok(s) => break s,
                    Err(e) => {
                        log::warn!("hub: cannot create pipe instance: {e}");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            };
            let client = std::mem::replace(&mut server, next);
            let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(serve(client, id, inner.clone()));
            continue;
        }
        // connect упал — пересоздаём экземпляр
        match create(false) {
            Ok(s) => server = s,
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

#[cfg(not(windows))]
async fn listen(_pipe: String, inner: Arc<Inner>) {
    inner.emit(HubEvent::ListenFailed("named pipes are only supported on Windows".into()));
}

async fn serve<S>(stream: S, id: SessionId, inner: Arc<Inner>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });

    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut chunk = vec![0u8; 16 * 1024];
    let mut hello_done = false;
    let started = tokio::time::Instant::now();

    'conn: loop {
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1]).trim().to_owned();
            if line.is_empty() {
                continue;
            }
            inner.traffic(id, false, &line);
            match modly_protocol::decode_client(&line) {
                Ok(ClientMsg::Hello { protocol, mod_info, game, pid, sdk }) if !hello_done => {
                    if protocol > PROTOCOL_VERSION {
                        let _ = tx.send(encode(&ServerMsg::Error {
                            text: format!("Мод использует протокол {protocol}, а Modly знает только {PROTOCOL_VERSION} — обновите Modly."),
                        }));
                        break 'conn;
                    }
                    {
                        let mut sessions = inner.sessions.lock().unwrap();
                        if sessions.len() >= MAX_SESSIONS {
                            let _ = tx.send(encode(&ServerMsg::Error { text: "Слишком много подключений".into() }));
                            break 'conn;
                        }
                        sessions.insert(id, tx.clone());
                    }
                    hello_done = true;
                    let lang = inner.lang.lock().unwrap().clone();
                    let welcome = encode(&ServerMsg::Welcome {
                        protocol: PROTOCOL_VERSION,
                        app: "Modly".into(),
                        version: inner.app_version.clone(),
                        lang,
                    });
                    inner.traffic(id, true, &welcome);
                    let _ = tx.send(welcome);
                    log::info!("hub: #{id} connected: {} {}", mod_info.id, mod_info.version);
                    inner.emit(HubEvent::Connected { id, hello: Hello { protocol, mod_info, game, pid, sdk } });
                }
                Ok(_) if !hello_done => {
                    let _ = tx.send(encode(&ServerMsg::Error { text: "Первым сообщением должно быть hello".into() }));
                    break 'conn;
                }
                Ok(ClientMsg::Bye) => break 'conn,
                Ok(ClientMsg::Hello { .. }) => {} // повторный hello игнорируем
                Ok(msg) => inner.emit(HubEvent::Message { id, msg }),
                Err(e) => {
                    log::debug!("hub: #{id} bad message: {e}");
                    if hello_done {
                        inner.emit(HubEvent::BadMessage { id, error: e.to_string(), line });
                    } else {
                        let _ = tx.send(encode(&ServerMsg::Error { text: format!("Не удалось разобрать hello: {e}") }));
                        break 'conn;
                    }
                }
            }
        }
        if buf.len() > MAX_MESSAGE {
            let _ = tx.send(encode(&ServerMsg::Error { text: "Слишком длинное сообщение".into() }));
            break;
        }
        // сессию закрыли со стороны Modly (disconnect)
        if hello_done && !inner.sessions.lock().unwrap().contains_key(&id) {
            break;
        }
        let read = if hello_done {
            tokio::time::timeout(Duration::from_secs(1), rd.read(&mut chunk)).await
        } else {
            let left = HELLO_TIMEOUT.saturating_sub(started.elapsed());
            if left.is_zero() {
                break;
            }
            tokio::time::timeout(left, rd.read(&mut chunk)).await
        };
        match read {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => continue, // таймаут: проверим, жива ли сессия, и ждём дальше
        }
    }

    inner.sessions.lock().unwrap().remove(&id);
    drop(tx);
    // дать писателю дослать последнее (например, error)
    let _ = tokio::time::timeout(Duration::from_millis(300), writer).await;
    if hello_done {
        log::info!("hub: #{id} disconnected");
        inner.emit(HubEvent::Disconnected { id });
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::windows::named_pipe::ClientOptions;

    fn pipe_name(tag: &str) -> String {
        format!(r"\\.\pipe\modly-test-{tag}-{}", std::process::id())
    }

    async fn open(name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
        for _ in 0..50 {
            if let Ok(c) = ClientOptions::new().open(name) {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("pipe {name} not available");
    }

    fn next(rx: &std_mpsc::Receiver<HubEvent>) -> HubEvent {
        loop {
            let e = rx.recv_timeout(Duration::from_secs(5)).expect("hub event");
            if !matches!(e, HubEvent::Listening | HubEvent::Traffic { .. }) {
                return e;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_lifecycle() {
        let name = pipe_name("life");
        let (etx, erx) = std_mpsc::channel();
        let hub = Hub::start_on(&tokio::runtime::Handle::current(), &name, "0.1.0", "ru", move |e| {
            let _ = etx.send(e);
        });

        let client = open(&name).await;
        let (rd, mut wr) = tokio::io::split(client);
        let mut lines = BufReader::new(rd).lines();
        wr.write_all(b"{\"type\":\"hello\",\"protocol\":1,\"mod\":{\"id\":\"demo\",\"name\":\"Demo\",\"version\":\"1\"},\"pid\":7}\n")
            .await
            .unwrap();
        let welcome = lines.next_line().await.unwrap().unwrap();
        assert!(welcome.contains("\"type\":\"welcome\"") && welcome.contains("\"lang\":\"ru\""), "{welcome}");

        let id = match tokio::task::block_in_place(|| next(&erx)) {
            HubEvent::Connected { id, hello } => {
                assert_eq!(hello.mod_info.id, "demo");
                assert_eq!(hello.pid, Some(7));
                id
            }
            other => panic!("{other:?}"),
        };

        // сообщения склеены в один write и одно разбито на части — разбор по строкам это переживает
        wr.write_all(b"{\"type\":\"state\",\"values\":{\"god\":true}}\n{\"type\":\"no").await.unwrap();
        wr.write_all(b"tify\",\"text\":\"hi\"}\nnot json\n").await.unwrap();
        assert!(matches!(tokio::task::block_in_place(|| next(&erx)), HubEvent::Message { msg: ClientMsg::State { .. }, .. }));
        assert!(matches!(tokio::task::block_in_place(|| next(&erx)), HubEvent::Message { msg: ClientMsg::Notify { .. }, .. }));
        assert!(matches!(tokio::task::block_in_place(|| next(&erx)), HubEvent::BadMessage { .. }));

        let req = hub.invoke(id, "heal");
        let line = lines.next_line().await.unwrap().unwrap();
        assert_eq!(line, format!("{{\"type\":\"invoke\",\"req\":{req},\"id\":\"heal\"}}"));

        hub.set_lang("en");
        let line = lines.next_line().await.unwrap().unwrap();
        assert_eq!(line, "{\"type\":\"lang\",\"lang\":\"en\"}");

        drop(wr);
        drop(lines);
        assert!(matches!(tokio::task::block_in_place(|| next(&erx)), HubEvent::Disconnected { .. }));
        assert_eq!(hub.session_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_messages_before_hello_and_newer_protocol() {
        let name = pipe_name("reject");
        let (etx, _erx) = std_mpsc::channel();
        let _hub = Hub::start_on(&tokio::runtime::Handle::current(), &name, "0.1.0", "en", move |e| {
            let _ = etx.send(e);
        });

        let client = open(&name).await;
        let (rd, mut wr) = tokio::io::split(client);
        let mut lines = BufReader::new(rd).lines();
        wr.write_all(b"{\"type\":\"state\",\"values\":{}}\n").await.unwrap();
        let err = lines.next_line().await.unwrap().unwrap();
        assert!(err.contains("\"type\":\"error\""), "{err}");

        let client = open(&name).await;
        let (rd, mut wr) = tokio::io::split(client);
        let mut lines = BufReader::new(rd).lines();
        wr.write_all(b"{\"type\":\"hello\",\"protocol\":99,\"mod\":{\"id\":\"x\",\"name\":\"X\"}}\n").await.unwrap();
        let err = lines.next_line().await.unwrap().unwrap();
        assert!(err.contains("\"type\":\"error\"") && err.contains("99"), "{err}");
    }
}
