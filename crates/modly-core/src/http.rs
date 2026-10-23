//! HTTP(S) через WinHTTP — клиент, встроенный в Windows.
//!
//! Сертификат сайта проверяет сама Windows (как браузер), прокси берётся системный, своей
//! библиотеки TLS в Modly нет. Перенаправления и cookies выключены: Modly ходит только по адресам,
//! которые построил сам, и сервер не может «увести» запрос в другое место. Исключение — запрос
//! с [`Request::follow`]: файлы выпусков GitHub отдаёт перенаправлением на свой CDN, и переход
//! разрешён только по https и только на перечисленные домены. Обычный http разрешён только для
//! своего компьютера (127.0.0.1 / localhost) — для проверки сайта локально.

use anyhow::{Result, bail};

/// Разобранный адрес: (https?, хост, порт, путь с запросом).
pub fn parse_url(url: &str) -> Result<(bool, String, u16, String)> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        bail!("адрес должен начинаться с https://");
    };
    let (hostport, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_owned()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_owned()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| anyhow::anyhow!("неверный порт в адресе"))?),
        None => (hostport, if secure { 443 } else { 80 }),
    };
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !host_ok {
        bail!("неверный адрес сервера");
    }
    if !secure && !is_loopback(host) {
        bail!("без https можно только к своему компьютеру");
    }
    if path.chars().any(|c| c.is_control() || c == ' ') {
        bail!("неверный адрес");
    }
    Ok((secure, host.to_owned(), port, path))
}

pub fn is_loopback(host: &str) -> bool {
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost")
}

/// Запрос. `headers` — дополнительные заголовки, `timeout_ms` — ожидание ответа.
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<&'a [u8]>,
    pub timeout_ms: i32,
    /// Куда можно перейти по перенаправлению (3xx): домены или `.суффикс` домена. Пусто — никуда.
    pub follow: &'static [&'static str],
}

/// Можно ли перейти на `url` по перенаправлению: только https и только на домен из списка.
pub fn redirect_allowed(url: &str, follow: &[&str]) -> bool {
    match parse_url(url) {
        Ok((true, host, _, _)) => {
            let host = host.to_ascii_lowercase();
            follow.iter().any(|f| match f.strip_prefix('.') {
                Some(suffix) => host.ends_with(&format!(".{suffix}")),
                None => host == *f,
            })
        }
        _ => false,
    }
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Self {
        Request { method: "GET", url, headers: Vec::new(), body: None, timeout_ms: 20_000, follow: &[] }
    }

    pub fn post_json(url: &'a str, body: &'a [u8]) -> Self {
        Request {
            method: "POST",
            url,
            headers: vec![("Content-Type", "application/json".to_owned())],
            body: Some(body),
            timeout_ms: 20_000,
            follow: &[],
        }
    }

    pub fn bearer(mut self, token: Option<&str>) -> Self {
        if let Some(t) = token {
            self.headers.push(("Authorization", format!("Bearer {t}")));
        }
        self
    }
}

/// Выполнить запрос. `on_head(статус, длина)` вызывается до тела (ошибка — прервать),
/// `on_data` — для каждого куска тела. Возвращает код ответа.
#[cfg(windows)]
pub fn send(
    req: &Request,
    on_head: &mut dyn FnMut(u16, Option<u64>) -> Result<()>,
    on_data: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<u16> {
    let mut url = req.url.to_owned();
    for _ in 0..5 {
        let hop = Request { url: &url, headers: req.headers.clone(), ..*req };
        match send_once(&hop, on_head, on_data)? {
            Sent::Done(status) => return Ok(status),
            Sent::Redirect(next) => {
                if !redirect_allowed(&next, req.follow) {
                    bail!("сервер перенаправил запрос на неразрешённый адрес");
                }
                url = next;
            }
        }
    }
    bail!("слишком много перенаправлений")
}

#[cfg(windows)]
enum Sent {
    Done(u16),
    Redirect(String),
}

#[cfg(windows)]
fn send_once(
    req: &Request,
    on_head: &mut dyn FnMut(u16, Option<u64>) -> Result<()>,
    on_data: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<Sent> {
    use std::ffi::c_void;
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::{HSTRING, PCWSTR};

    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    fn check(h: *mut c_void, what: &str) -> Result<Handle> {
        if h.is_null() {
            bail!("{what}: {}", windows::core::Error::from_thread().message());
        }
        Ok(Handle(h))
    }
    fn net_err(e: windows::core::Error) -> anyhow::Error {
        // Коды WinHTTP (12xxx) понятнее словами.
        let code = (e.code().0 as u32) & 0xFFFF;
        let text = match code {
            12002 => "сервер не ответил вовремя",
            12007 => "сервер не найден — проверьте интернет",
            12029 | 12030 => "не удалось подключиться к серверу",
            12175 | 12045 | 12037 | 12038 | 12044 => "сертификат сайта не прошёл проверку",
            _ => "",
        };
        if text.is_empty() { anyhow::anyhow!("сеть: {}", e.message()) } else { anyhow::anyhow!("{text}") }
    }

    let (secure, host, port, path) = parse_url(req.url)?;
    unsafe {
        let agent = HSTRING::from(format!("Modly/{}", env!("CARGO_PKG_VERSION")));
        let session = check(
            WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "WinHttpOpen",
        )?;
        // Только TLS 1.2 и 1.3 (1.3 есть не во всех сборках Windows 10 — тогда остаётся 1.2).
        let tls = (WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3).to_ne_bytes();
        if WinHttpSetOption(Some(session.0), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&tls)).is_err() {
            let _ = WinHttpSetOption(Some(session.0), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2.to_ne_bytes()));
        }
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 60_000, req.timeout_ms).map_err(net_err)?;

        let connect = check(WinHttpConnect(session.0, &HSTRING::from(host.as_str()), port, 0), "WinHttpConnect")?;
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = check(
            WinHttpOpenRequest(
                connect.0,
                &HSTRING::from(req.method),
                &HSTRING::from(path.as_str()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                flags,
            ),
            "WinHttpOpenRequest",
        )?;
        let off = (WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS).to_ne_bytes();
        WinHttpSetOption(Some(request.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&off)).map_err(net_err)?;
        WinHttpSetOption(Some(request.0), WINHTTP_OPTION_REDIRECT_POLICY, Some(&WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes()))
            .map_err(net_err)?;

        let mut head = if req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept")) {
            String::new()
        } else {
            String::from("Accept: application/json, */*\r\n")
        };
        for (k, v) in &req.headers {
            if v.contains(['\r', '\n']) {
                bail!("неверный заголовок");
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        let head: Vec<u16> = head.encode_utf16().collect();
        let body = req.body.unwrap_or(&[]);
        WinHttpSendRequest(
            request.0,
            Some(&head),
            (!body.is_empty()).then_some(body.as_ptr().cast::<c_void>()),
            body.len() as u32,
            body.len() as u32,
            0,
        )
        .map_err(net_err)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(net_err)?;

        let mut status: u32 = 0;
        let mut len = 4u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(net_err)?;
        if !req.follow.is_empty() && matches!(status, 301 | 302 | 303 | 307 | 308) {
            let mut buf = vec![0u16; 4096];
            let mut size = (buf.len() * 2) as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_LOCATION,
                PCWSTR::null(),
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                std::ptr::null_mut(),
            )
            .map_err(net_err)?;
            return Ok(Sent::Redirect(String::from_utf16_lossy(&buf[..size as usize / 2])));
        }
        let mut clen: u32 = 0;
        let mut len = 4u32;
        let has_len = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut clen as *mut u32).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
        .is_ok();
        on_head(status as u16, has_len.then_some(clen as u64))?;

        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut read).map_err(net_err)?;
            if read == 0 {
                break;
            }
            on_data(&buf[..read as usize])?;
        }
        Ok(Sent::Done(status as u16))
    }
}

#[cfg(not(windows))]
pub fn send(
    _req: &Request,
    _on_head: &mut dyn FnMut(u16, Option<u64>) -> Result<()>,
    _on_data: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<u16> {
    bail!("сеть пока есть только в Windows-сборке Modly")
}

/// Запрос целиком в память (не больше `limit` байт). Возвращает (код, тело).
pub fn fetch(req: &Request, limit: usize) -> Result<(u16, Vec<u8>)> {
    let mut body = Vec::new();
    let status = send(req, &mut |_, len| {
        if len.is_some_and(|l| l as usize > limit) {
            bail!("ответ сервера слишком большой");
        }
        Ok(())
    }, &mut |chunk| {
        if body.len() + chunk.len() > limit {
            bail!("ответ сервера слишком большой");
        }
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_safe_urls() {
        assert_eq!(
            parse_url("https://mods.hackerstone.xyz/api/v1/mods?q=a").unwrap(),
            (true, "mods.hackerstone.xyz".into(), 443, "/api/v1/mods?q=a".into())
        );
        assert_eq!(parse_url("http://127.0.0.1:8082/x").unwrap(), (false, "127.0.0.1".into(), 8082, "/x".into()));
        assert_eq!(parse_url("https://example.com?a=1").unwrap().3, "/?a=1");
        for bad in [
            "http://example.com/",
            "ftp://x/",
            "https://user@evil.com/",
            "https://a b/",
            "https://x.com:99999/",
            "https:///path",
            "https://x.com/a b",
            "https://x.com/\r\nHost: y",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn redirects_only_to_listed_https_hosts() {
        let gh: &[&str] = &["github.com", ".githubusercontent.com"];
        assert!(redirect_allowed("https://github.com/a/b/releases/download/v1/x.zip", gh));
        assert!(redirect_allowed("https://release-assets.githubusercontent.com/x?sig=1", gh));
        assert!(!redirect_allowed("http://objects.githubusercontent.com/x", gh));
        assert!(!redirect_allowed("https://evilgithub.com/x", gh));
        assert!(!redirect_allowed("https://githubusercontent.com.evil.io/x", gh));
        assert!(!redirect_allowed("https://github.com/x", &[]));
    }
}
