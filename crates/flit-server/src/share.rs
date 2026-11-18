//! The browser share: files handed to a computer that has nothing installed.
//!
//! The sending device serves. A person picks files, the app binds this listener
//! for as long as the share is open, and the page at the other end only reads.
//! There is no upload path, no session state to resume and no way to ask for a
//! file that was not chosen.
//!
//! Deliberately not hyper. A shared file can be gigabytes, and the three
//! responses here are small enough to write by hand: a bounded request head, a
//! fixed set of headers, then bytes copied straight out of the file. Nothing is
//! parsed into a request object that could be mistaken for an upload, and no
//! body is ever read.
//!
//! The token is 128 random bits. The first address that presents it claims the
//! share, so a URL that leaks to the next desk stops working for everyone else;
//! the share also stops on its own after [`DEFAULT_SHARE_TTL`], when the person
//! closes it, or when the browser posts `finish`. Every failure is the same
//! `404`, because a distinguishable "wrong address" or "expired" would tell a
//! prober which part of a guess was right.

use std::{
    future::Future,
    io::{self, SeekFrom},
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};

/// Port the browser share listens on. See `docs/protocol.md` §2.
pub const SHARE_PORT: u16 = 47822;

/// How long a share stays open unless the person closes it first.
pub const DEFAULT_SHARE_TTL: Duration = Duration::from_secs(10 * 60);

/// The most files one share may hold.
///
/// A browser page listing thousands of links helps nobody, and the page is held
/// in memory in full. Larger selections belong in a real send.
pub const MAX_SHARE_FILES: usize = 200;

/// Largest request head accepted, in bytes.
///
/// Only the request line and `Range` matter here, so anything longer is either a
/// mistake or someone probing for a parser bug.
const MAX_HEAD: usize = 8 * 1024;

/// How much of a file is read at a time when it is copied to the socket.
const COPY_CHUNK: usize = 64 * 1024;

/// One file in a share.
#[derive(Clone, Debug)]
pub struct ShareFile {
    /// Name shown on the page and used for the download.
    pub name: String,
    /// Where the bytes are read from.
    pub path: PathBuf,
    /// Length reported to the browser.
    pub size: u64,
}

impl ShareFile {
    /// Describe a file on disk.
    ///
    /// The length is read once, when the share opens. A file that grows or
    /// shrinks afterwards is served as far as the recorded length allows; the
    /// page is a snapshot, not a live directory.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ShareError> {
        let path = path.into();
        let size = std::fs::metadata(&path)
            .map_err(|source| ShareError::File {
                path: path.clone(),
                source,
            })?
            .len();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());
        Ok(Self { name, path, size })
    }
}

/// Things that stop a share from opening.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    /// A chosen file could not be read.
    #[error("could not read {path}: {source}")]
    File {
        /// The file that failed.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: io::Error,
    },
    /// Nothing was chosen.
    #[error("there is nothing to share")]
    Empty,
    /// Too many files for one page.
    #[error("{count} files is more than one browser share can hold (max {max})")]
    TooMany {
        /// How many were offered.
        count: usize,
        /// The cap.
        max: usize,
    },
    /// The port could not be bound.
    #[error("could not open the browser share on {addr}: {source}")]
    Bind {
        /// The address that was refused.
        addr: SocketAddr,
        /// The operating system's reason.
        #[source]
        source: io::Error,
    },
    /// The system random source failed.
    #[error("could not make a share token: {0}")]
    Random(getrandom::Error),
    /// A socket operation failed after the share was bound.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A bound, read-only share waiting to be served.
pub struct BrowserShare {
    listener: TcpListener,
    state: Arc<ShareState>,
    local_addr: SocketAddr,
}

impl BrowserShare {
    /// Bind a share on `bind` that stays open for [`DEFAULT_SHARE_TTL`].
    pub async fn open(
        alias: impl Into<String>,
        files: Vec<ShareFile>,
        bind: SocketAddr,
    ) -> Result<Self, ShareError> {
        Self::open_with_ttl(alias, files, bind, DEFAULT_SHARE_TTL).await
    }

    /// Bind a share with an explicit lifetime. Tests use this; the app does not.
    pub async fn open_with_ttl(
        alias: impl Into<String>,
        files: Vec<ShareFile>,
        bind: SocketAddr,
        ttl: Duration,
    ) -> Result<Self, ShareError> {
        if files.is_empty() {
            return Err(ShareError::Empty);
        }
        if files.len() > MAX_SHARE_FILES {
            return Err(ShareError::TooMany {
                count: files.len(),
                max: MAX_SHARE_FILES,
            });
        }
        let listener = TcpListener::bind(bind)
            .await
            .map_err(|source| ShareError::Bind { addr: bind, source })?;
        let local_addr = listener.local_addr()?;
        Ok(Self {
            listener,
            local_addr,
            state: Arc::new(ShareState {
                alias: alias.into(),
                files,
                token: new_token()?,
                deadline: Instant::now() + ttl,
                claimed: Mutex::new(None),
                finished: Notify::new(),
                done: AtomicBool::new(false),
            }),
        })
    }

    /// The address actually bound.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The capability in the URL. Shown only to the person who opened the share.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.state.token
    }

    /// The address to read out or draw as a code.
    ///
    /// Uses the bound address when it names one, and otherwise the interface the
    /// default route would use. A machine with no LAN route falls back to
    /// loopback, which at least works for a browser on the same machine.
    #[must_use]
    pub fn url(&self) -> String {
        let host = match self.local_addr.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() => ip.to_string(),
            IpAddr::V4(_) => crate::local_ipv4_addrs()
                .first()
                .map(ToString::to_string)
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            IpAddr::V6(ip) if !ip.is_unspecified() => format!("[{ip}]"),
            IpAddr::V6(_) => "[::1]".to_string(),
        };
        format!(
            "http://{host}:{}/r/{}",
            self.local_addr.port(),
            self.state.token
        )
    }

    /// How long is left before the share closes on its own.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.state
            .deadline
            .saturating_duration_since(Instant::now())
    }

    /// The files being shared, in page order.
    #[must_use]
    pub fn files(&self) -> &[ShareFile] {
        &self.state.files
    }

    /// Serve until the share expires, the browser posts `finish`, or `shutdown`
    /// resolves. In every case the listener closes.
    pub async fn run_until<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);
        let expires = tokio::time::Instant::from_std(self.state.deadline);
        loop {
            tokio::select! {
                () = &mut shutdown => return Ok(()),
                () = tokio::time::sleep_until(expires) => {
                    log::info!("browser share closed after its time ran out");
                    return Ok(());
                }
                () = self.state.finished.notified() => {
                    log::info!("browser share closed by the browser");
                    return Ok(());
                }
                accepted = self.listener.accept() => {
                    match accepted {
                        Ok((stream, peer)) => {
                            let state = Arc::clone(&self.state);
                            tokio::spawn(async move {
                                if let Err(err) = serve(stream, peer.ip(), state).await {
                                    log::debug!("browser share connection ended: {err}");
                                }
                            });
                        }
                        // One failed accept must not close the share.
                        Err(err) => {
                            log::warn!("browser share accept failed: {err}");
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    }

    /// Serve until something closes the share.
    pub async fn run(self) -> io::Result<()> {
        self.run_until(std::future::pending()).await
    }
}

struct ShareState {
    alias: String,
    files: Vec<ShareFile>,
    token: String,
    deadline: Instant,
    claimed: Mutex<Option<IpAddr>>,
    finished: Notify,
    done: AtomicBool,
}

impl ShareState {
    /// Whether `peer` may use this share, claiming it if nobody has yet.
    ///
    /// The claim is taken on the first request that presents the token, page or
    /// file. A wrong address is refused from then on, which is the point: a
    /// token that leaked stops being a second key.
    fn authorize(&self, token: &str, peer: IpAddr) -> bool {
        if Instant::now() >= self.deadline {
            return false;
        }
        if !token_matches(&self.token, token) {
            return false;
        }
        let mut claimed = self.claimed.lock().expect("share claim lock");
        match *claimed {
            None => {
                *claimed = Some(peer);
                true
            }
            Some(owner) => owner == peer,
        }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    fn minutes_left(&self) -> u64 {
        let left = self.deadline.saturating_duration_since(Instant::now());
        left.as_secs().div_ceil(60).max(1)
    }

    fn page_html(&self) -> String {
        let count = self.files.len();
        let noun = if count == 1 { "file" } else { "files" };
        let total: u64 = self.files.iter().map(|file| file.size).sum();
        let minutes = self.minutes_left();
        let mut rows = String::new();
        for (index, file) in self.files.iter().enumerate() {
            rows.push_str(&format!(
                "<li><a href=\"/r/{token}/{index}\">\
                 <span class=\"name\">{name}</span>\
                 <span class=\"size\">{size}</span></a></li>",
                token = self.token,
                name = html_escape(&file.name),
                size = human_size(file.size),
            ));
        }
        format!(
            "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"color-scheme\" content=\"light dark\">\n\
<meta name=\"referrer\" content=\"no-referrer\">\n\
<title>{alias} is sharing {count} {noun}</title>\n\
<style>{style}</style>\n\
</head>\n\
<body>\n\
<main>\n\
<p class=\"eyebrow\">Flit</p>\n\
<h1>{alias} is sharing {count} {noun} with this computer</h1>\n\
<p class=\"lede\">Download {them} below, {total}. Nothing is installed, and \
nothing is uploaded.</p>\n\
<ul class=\"files\">{rows}</ul>\n\
<p class=\"note\">Anyone with this address can download these for the next \
{minutes} {minute_word}. Keep this tab open until the download finishes.</p>\n\
<form method=\"post\" action=\"/r/{token}/finish\">\
<button type=\"submit\">Done</button></form>\n\
</main>\n\
</body>\n\
</html>\n",
            alias = html_escape(&self.alias),
            count = count,
            noun = noun,
            style = PAGE_STYLE,
            them = if count == 1 { "it" } else { "them" },
            total = human_size(total),
            rows = rows,
            minutes = minutes,
            minute_word = if minutes == 1 { "minute" } else { "minutes" },
            token = self.token,
        )
    }

    fn done_html(&self) -> String {
        format!(
            "<!doctype html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"color-scheme\" content=\"light dark\">\n\
<title>Share closed</title>\n\
<style>{style}</style>\n\
</head>\n\
<body>\n\
<main>\n\
<p class=\"eyebrow\">Flit</p>\n\
<h1>Share closed</h1>\n\
<p class=\"lede\">The files are no longer being offered. {alias} can open a \
new share from the phone whenever they are ready.</p>\n\
</main>\n\
</body>\n\
</html>\n",
            style = PAGE_STYLE,
            alias = html_escape(&self.alias),
        )
    }
}

/// The page's whole appearance, inlined so it loads nothing else.
const PAGE_STYLE: &str = "\
*,*::before,*::after{box-sizing:border-box}\
body{margin:0;min-height:100dvh;display:grid;place-items:center;padding:24px;\
background:#f4f2ee;color:#14181c;\
font:16px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif;\
-webkit-font-smoothing:antialiased}\
main{width:100%;max-width:34rem;background:#fff;border-radius:20px;padding:2rem;\
box-shadow:0 1px 2px rgba(20,24,28,.06),0 12px 32px -12px rgba(20,24,28,.18)}\
.eyebrow{margin:0 0 1.25rem;font-size:.75rem;font-weight:600;letter-spacing:.08em;\
text-transform:uppercase;color:#0b6b4f}\
h1{margin:0;font-size:1.5rem;line-height:1.2;letter-spacing:-.015em;font-weight:600}\
.lede{margin:.625rem 0 0;color:#4a525a;font-size:.9375rem}\
.files{list-style:none;margin:1.75rem 0 0;padding:0;border-top:1px solid #eceae5}\
.files li{border-bottom:1px solid #eceae5}\
.files a{display:flex;align-items:baseline;gap:1rem;padding:.875rem 0;\
color:inherit;text-decoration:none;border-radius:6px}\
.files a:hover .name{text-decoration:underline}\
.files a:focus-visible{outline:2px solid #0b6b4f;outline-offset:3px}\
.name{flex:1;min-width:0;overflow-wrap:anywhere;font-size:.9375rem;font-weight:500}\
.size{color:#666e78;font-size:.8125rem;font-variant-numeric:tabular-nums}\
.note{margin:1.5rem 0 0;color:#666e78;font-size:.8125rem}\
form{margin:1.25rem 0 0}\
button{font:inherit;font-size:.875rem;font-weight:500;color:#0b6b4f;\
background:none;border:0;padding:.25rem 0;cursor:pointer}\
button:hover{text-decoration:underline}\
button:focus-visible{outline:2px solid #0b6b4f;outline-offset:3px}\
@media (prefers-color-scheme:dark){\
body{background:#101418;color:#eef1f4}\
main{background:#171c21;box-shadow:0 1px 2px rgba(0,0,0,.4),\
0 16px 40px -16px rgba(0,0,0,.6)}\
.eyebrow,button{color:#6fd3ad}\
.lede,.note,.size{color:#9aa4ae}\
.files,.files li{border-color:#232a31}\
.files a:focus-visible,button:focus-visible{outline-color:#6fd3ad}}\
@media (prefers-contrast:more){\
main{border:1px solid #14181c}\
.lede,.note,.size{color:#30373d}\
@media (prefers-color-scheme:dark){main{border-color:#eef1f4}\
.lede,.note,.size{color:#c9d2da}}}\
";

/// One parsed request line, plus the only header this server reads.
struct Request<'a> {
    method: &'a str,
    target: &'a str,
    range: Option<&'a str>,
}

async fn serve(mut stream: TcpStream, peer: IpAddr, state: Arc<ShareState>) -> io::Result<()> {
    let _ = stream.set_nodelay(true);
    let head = read_head(&mut stream).await?;
    let Some(request) = parse_head(&head) else {
        return not_found(&mut stream, false).await;
    };
    let head_only = request.method == "HEAD";
    let Some((token, tail)) = share_path(request.target) else {
        return not_found(&mut stream, head_only).await;
    };
    // Every refusal is the same 404. A wrong address, an expired share and a
    // guessed token must be indistinguishable, or the page becomes an oracle.
    if !state.authorize(token, peer) {
        return not_found(&mut stream, head_only).await;
    }

    match (request.method, tail) {
        ("GET" | "HEAD", None) => {
            let page = state.page_html();
            respond(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
                head_only,
                &[],
            )
            .await
        }
        ("GET" | "HEAD", Some(index)) => {
            let Some(index) = index.parse::<usize>().ok() else {
                return not_found(&mut stream, head_only).await;
            };
            let Some(file) = state.files.get(index).cloned() else {
                return not_found(&mut stream, head_only).await;
            };
            serve_file(&mut stream, &file, request.range, head_only).await
        }
        ("POST", Some("finish")) => {
            if state.is_done() {
                return not_found(&mut stream, head_only).await;
            }
            state.done.store(true, Ordering::Relaxed);
            let page = state.done_html();
            respond(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
                head_only,
                &[],
            )
            .await?;
            state.finished.notify_one();
            Ok(())
        }
        _ => not_found(&mut stream, head_only).await,
    }
}

/// Serve one file, honouring a single `Range`.
async fn serve_file(
    stream: &mut TcpStream,
    file: &ShareFile,
    range: Option<&str>,
    head_only: bool,
) -> io::Result<()> {
    let (start, length, partial) = match range {
        None => (0, file.size, false),
        Some(value) => match parse_range(value, file.size) {
            Some(Ok((first, last))) => (first, last - first + 1, true),
            Some(Err(())) => {
                let headers = [(
                    "Content-Range".to_string(),
                    format!("bytes */{}", file.size),
                )];
                return respond(
                    stream,
                    416,
                    "text/plain; charset=utf-8",
                    b"",
                    true,
                    &headers,
                )
                .await;
            }
            // A shape this server does not answer (several ranges, another
            // unit) is served whole, which every client understands.
            None => (0, file.size, false),
        },
    };

    let mut handle = match tokio::fs::File::open(&file.path).await {
        Ok(handle) => handle,
        Err(err) => {
            log::warn!(
                "shared file {} could not be opened: {err}",
                file.path.display()
            );
            return not_found(stream, head_only).await;
        }
    };
    handle.seek(SeekFrom::Start(start)).await?;

    let mut headers = vec![
        ("Content-Disposition".to_string(), disposition(&file.name)),
        ("Accept-Ranges".to_string(), "bytes".to_string()),
    ];
    if partial {
        headers.push((
            "Content-Range".to_string(),
            format!(
                "bytes {start}-{}/{size}",
                start + length - 1,
                size = file.size
            ),
        ));
    }
    write_head(
        stream,
        if partial { 206 } else { 200 },
        "application/octet-stream",
        length,
        &headers,
    )
    .await?;
    if head_only {
        return Ok(());
    }

    let mut remaining = length;
    let mut buffer = vec![0u8; COPY_CHUNK];
    while remaining > 0 {
        let take = remaining.min(buffer.len() as u64) as usize;
        let read = handle.read(&mut buffer[..take]).await?;
        if read == 0 {
            // The file shrank under the page. The length promised is already on
            // the wire, so the browser sees a short read and retries; saying so
            // is more useful than pretending the bytes were sent.
            log::warn!("{} ended before its recorded length", file.path.display());
            break;
        }
        stream.write_all(&buffer[..read]).await?;
        remaining -= read as u64;
    }
    Ok(())
}

/// Read at most [`MAX_HEAD`] bytes, stopping at the end of the request head.
async fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if head.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(head);
        }
        if head.len() >= MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(head);
        }
        head.extend_from_slice(&chunk[..read]);
    }
}

fn parse_head(head: &[u8]) -> Option<Request<'_>> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?;
    let target = request_line.next()?;
    request_line.next()?; // the HTTP version is not interesting here
    let mut range = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(value.trim());
            }
        }
    }
    Some(Request {
        method,
        target,
        range,
    })
}

/// Split `/r/{token}` and `/r/{token}/{tail}` without letting anything else in.
fn share_path(target: &str) -> Option<(&str, Option<&str>)> {
    let path = target.split('?').next()?;
    let rest = path.strip_prefix("/r/")?;
    if rest.is_empty() {
        return None;
    }
    match rest.split_once('/') {
        Some((token, tail)) if !token.is_empty() && !tail.is_empty() => Some((token, Some(tail))),
        Some(_) => None,
        None => Some((rest, None)),
    }
}

fn new_token() -> Result<String, ShareError> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(ShareError::Random)?;
    Ok(hex::encode(bytes))
}

/// Compare two tokens without leaking where they first differ.
fn token_matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    if expected.len() != presented.len() {
        return false;
    }
    expected
        .iter()
        .zip(presented)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// The byte window a `Range` header asks for, as inclusive bounds.
///
/// `None` means "serve the whole file" (another unit, or several ranges).
/// `Some(Err(()))` means the range was understood and cannot be satisfied.
fn parse_range(value: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let value = value.trim();
    let list = value.strip_prefix("bytes=")?.trim();
    if list.contains(',') {
        return None;
    }
    let (first, last) = list.split_once('-')?;
    let (first, last) = (first.trim(), last.trim());
    if size == 0 {
        return Some(Err(()));
    }
    let (start, end) = if first.is_empty() {
        let suffix: u64 = last.parse().ok()?;
        if suffix == 0 {
            return Some(Err(()));
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = first.parse().ok()?;
        let end = if last.is_empty() {
            size - 1
        } else {
            last.parse::<u64>().ok()?.min(size - 1)
        };
        (start, end)
    };
    if start >= size || start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

async fn not_found(stream: &mut TcpStream, head_only: bool) -> io::Result<()> {
    respond(
        stream,
        404,
        "text/plain; charset=utf-8",
        b"not found",
        head_only,
        &[],
    )
    .await
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head_only: bool,
    extra: &[(String, String)],
) -> io::Result<()> {
    write_head(stream, status, content_type, body.len() as u64, extra).await?;
    if !head_only {
        stream.write_all(body).await?;
        stream.flush().await?;
    }
    Ok(())
}

async fn write_head(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    length: u64,
    extra: &[(String, String)],
) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\n\
Content-Type: {content_type}\r\n\
Content-Length: {length}\r\n\
X-Content-Type-Options: nosniff\r\n\
Cache-Control: no-store\r\n\
Referrer-Policy: no-referrer\r\n\
Connection: close\r\n",
        status_text(status)
    );
    // The page is the only HTML served, and it is built from names a person
    // chose. Forbidding every fetch means a name that somehow slipped through
    // escaping still has nowhere to send anything.
    if content_type.starts_with("text/html") {
        head.push_str(
            "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'\r\n",
        );
    }
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    Ok(())
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "OK",
    }
}

/// A `Content-Disposition` that survives any script and any quote.
fn disposition(name: &str) -> String {
    let fallback: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || " ._-".contains(character) {
                character
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
                (*byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < units.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", units[unit])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A scratch directory that does not need a temp-dir crate.
    fn scratch(name: &str) -> PathBuf {
        let unique = format!(
            "flit-share-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(root: &Path, name: &str, bytes: &[u8]) -> ShareFile {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        ShareFile::new(path).unwrap()
    }

    fn state(root: &Path, alias: &str, files: &[(&str, &[u8])], ttl: Duration) -> Arc<ShareState> {
        state_from(
            alias,
            files
                .iter()
                .map(|(name, bytes)| file(root, name, bytes))
                .collect(),
            ttl,
        )
    }

    /// A share over names that never touch a disk.
    ///
    /// `page_html` only reads a file's name and length, and Windows refuses to
    /// create names that are fine on the wire (`<script>…`, `:`, `?`), so the
    /// escaping tests build the entries directly.
    fn state_from(alias: &str, files: Vec<ShareFile>, ttl: Duration) -> Arc<ShareState> {
        Arc::new(ShareState {
            alias: alias.to_string(),
            files,
            token: "0123456789abcdef0123456789abcdef".to_string(),
            deadline: Instant::now() + ttl,
            claimed: Mutex::new(None),
            finished: Notify::new(),
            done: AtomicBool::new(false),
        })
    }

    fn named(name: &str, size: u64) -> ShareFile {
        ShareFile {
            name: name.to_string(),
            path: PathBuf::from(name),
            size,
        }
    }

    fn address(text: &str) -> IpAddr {
        text.parse().expect("a literal address")
    }

    async fn exchange(addr: SocketAddr, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("read");
        response
    }

    fn text(response: &[u8]) -> String {
        String::from_utf8_lossy(response).into_owned()
    }

    fn body(response: &[u8]) -> Vec<u8> {
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("a head and a body");
        response[split + 4..].to_vec()
    }

    #[test]
    fn a_token_is_thirty_two_hex_characters() {
        let token = new_token().unwrap();
        assert_eq!(token.len(), 32, "128 bits in hex");
        assert!(token.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(token, new_token().unwrap(), "tokens are not reused");
    }

    /// A name is a claim from another device, so the page must not be able to
    /// run it. Escaping is the only thing between a file name and the markup.
    #[test]
    fn the_page_lists_every_file_and_escapes_a_name() {
        let share = state_from(
            "Rudra's phone",
            vec![
                named("notes.txt", 5),
                named("<script>alert(1)</script>.txt", 1),
            ],
            DEFAULT_SHARE_TTL,
        );
        let page = share.page_html();
        assert!(page.contains("notes.txt"));
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;.txt"));
        assert!(!page.contains("<script>"), "no name becomes an element");
        assert!(page.contains("2 files"), "the count is stated once");
        assert!(page.contains("next 10 minutes"));
        assert!(
            page.contains("Rudra&#39;s phone"),
            "the alias is escaped too"
        );
    }

    /// The page must stand on its own: no script, no font, no analytics. A
    /// borrowed computer may have no internet at all.
    #[test]
    fn the_page_loads_nothing_from_anywhere() {
        let root = scratch("offline");
        let share = state(&root, "phone", &[("a.txt", b"a")], DEFAULT_SHARE_TTL);
        let page = share.page_html();
        assert!(!page.contains("<script"));
        assert!(!page.contains("http://"));
        assert!(!page.contains("https://"));
        assert!(!page.contains("<link"));
        assert!(!page.contains("<img"));
    }

    #[test]
    fn one_address_claims_the_token_and_others_get_nothing() {
        let root = scratch("claim");
        let share = state(&root, "phone", &[("a.txt", b"a")], DEFAULT_SHARE_TTL);
        let token = share.token.clone();
        assert!(share.authorize(&token, address("10.0.0.2")));
        assert!(
            !share.authorize(&token, address("10.0.0.3")),
            "a link that reached another desk is not a second key"
        );
        assert!(
            !share.authorize("ffffffffffffffffffffffffffffffff", address("10.0.0.2")),
            "the owner still needs the right token"
        );
        assert!(
            share.authorize(&token, address("10.0.0.2")),
            "a bad attempt does not lock the owner out"
        );
    }

    #[test]
    fn an_expired_share_refuses_every_address() {
        let root = scratch("expired");
        let share = state(&root, "phone", &[("a.txt", b"a")], Duration::ZERO);
        let token = share.token.clone();
        assert!(!share.authorize(&token, address("10.0.0.2")));
        assert!(!share.authorize(&token, address("10.0.0.2")));
    }

    #[test]
    fn a_range_is_translated_into_a_slice() {
        assert_eq!(parse_range("bytes=0-3", 10), Some(Ok((0, 3))));
        assert_eq!(parse_range("bytes=5-", 10), Some(Ok((5, 9))));
        assert_eq!(parse_range("bytes=-3", 10), Some(Ok((7, 9))));
        assert_eq!(
            parse_range("bytes=0-99", 10),
            Some(Ok((0, 9))),
            "a range past the end is clamped, not refused"
        );
        assert_eq!(parse_range("bytes=10-", 10), Some(Err(())));
        assert_eq!(parse_range("bytes=5-4", 10), Some(Err(())));
        assert_eq!(parse_range("bytes=-0", 10), Some(Err(())));
        assert_eq!(parse_range("items=0-1", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
        assert_eq!(parse_range("bytes=0-", 0), Some(Err(())));
    }

    #[test]
    fn a_disposition_keeps_the_name_but_never_the_quotes() {
        let value = disposition("a\"b\u{00e9}.txt");
        assert!(value.starts_with("attachment; filename=\"a_b_.txt\""));
        assert!(value.contains("filename*=UTF-8''a%22b%C3%A9.txt"));
    }

    #[test]
    fn sizes_are_stated_in_units_a_person_reads() {
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1500), "1.5 KB");
        assert_eq!(human_size(1_000_000), "1.0 MB");
        assert_eq!(human_size(2_500_000_000), "2.5 GB");
    }

    #[tokio::test]
    async fn a_share_reports_a_url_that_carries_its_token() {
        let root = scratch("url");
        let share = BrowserShare::open(
            "phone",
            vec![file(&root, "a.txt", b"a")],
            SocketAddr::from(([127, 0, 0, 1], 0)),
        )
        .await
        .unwrap();
        let url = share.url();
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert!(url.ends_with(share.token()), "{url}");
        assert!(share.remaining() <= DEFAULT_SHARE_TTL);
        assert_eq!(share.files().len(), 1);
        drop(share);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_share_needs_a_file_and_a_page_sized_list() {
        let empty =
            BrowserShare::open("phone", vec![], SocketAddr::from(([127, 0, 0, 1], 0))).await;
        assert!(matches!(empty, Err(ShareError::Empty)));

        let too_many: Vec<ShareFile> = (0..=MAX_SHARE_FILES)
            .map(|index| ShareFile {
                name: format!("file-{index}"),
                path: PathBuf::from("not-read"),
                size: 0,
            })
            .collect();
        let refused =
            BrowserShare::open("phone", too_many, SocketAddr::from(([127, 0, 0, 1], 0))).await;
        assert!(matches!(refused, Err(ShareError::TooMany { .. })));
    }

    /// The whole flow over a real socket: page, a resumed download, a whole
    /// download, a head request, an index that does not exist, and the receipt
    /// that closes the share.
    #[tokio::test]
    async fn a_share_serves_a_page_a_file_and_closes_on_finish() {
        let root = scratch("wire");
        let share = BrowserShare::open(
            "Test phone",
            vec![file(&root, "note.txt", b"hello world")],
            SocketAddr::from(([127, 0, 0, 1], 0)),
        )
        .await
        .unwrap();
        let addr = share.local_addr();
        let token = share.token().to_string();
        let serving = tokio::spawn(async move { share.run().await });

        let page = exchange(addr, &format!("GET /r/{token} HTTP/1.1\r\nHost: x\r\n\r\n")).await;
        let page_text = text(&page);
        assert!(page_text.starts_with("HTTP/1.1 200"), "{page_text}");
        assert!(page_text.contains("Cache-Control: no-store"));
        assert!(page_text.contains("Content-Security-Policy: default-src 'none'"));
        assert!(text(&body(&page)).contains("note.txt"));

        let ranged = exchange(
            addr,
            &format!("GET /r/{token}/0 HTTP/1.1\r\nHost: x\r\nRange: bytes=6-10\r\n\r\n"),
        )
        .await;
        let ranged_text = text(&ranged);
        assert!(ranged_text.starts_with("HTTP/1.1 206"), "{ranged_text}");
        assert!(ranged_text.contains("Content-Range: bytes 6-10/11"));
        assert!(ranged_text.contains("Accept-Ranges: bytes"));
        assert_eq!(body(&ranged), b"world");

        let whole = exchange(
            addr,
            &format!("GET /r/{token}/0 HTTP/1.1\r\nHost: x\r\n\r\n"),
        )
        .await;
        assert_eq!(body(&whole), b"hello world");

        let headed = exchange(
            addr,
            &format!("HEAD /r/{token}/0 HTTP/1.1\r\nHost: x\r\n\r\n"),
        )
        .await;
        assert!(body(&headed).is_empty(), "a head request has no body");
        assert!(text(&headed).contains("Content-Length: 11"));

        let missing = exchange(
            addr,
            &format!("GET /r/{token}/9 HTTP/1.1\r\nHost: x\r\n\r\n"),
        )
        .await;
        assert!(text(&missing).starts_with("HTTP/1.1 404"));
        assert_eq!(body(&missing), b"not found");

        let done = exchange(
            addr,
            &format!("POST /r/{token}/finish HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"),
        )
        .await;
        assert!(text(&done).starts_with("HTTP/1.1 200"));
        assert!(text(&body(&done)).contains("Share closed"));

        let stopped = tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("the listener closes on finish");
        assert!(stopped.unwrap().is_ok());

        std::fs::remove_dir_all(&root).ok();
    }

    /// A guessed or expired token must not reveal that the port is a share at
    /// all: same status, same body, no file names.
    #[tokio::test]
    async fn an_unknown_token_is_a_404_that_says_nothing() {
        let root = scratch("unknown");
        let share = BrowserShare::open(
            "Test phone",
            vec![file(&root, "secret.txt", b"private")],
            SocketAddr::from(([127, 0, 0, 1], 0)),
        )
        .await
        .unwrap();
        let addr = share.local_addr();
        let serving = tokio::spawn(async move { share.run().await });

        let response = exchange(
            addr,
            "GET /r/ffffffffffffffffffffffffffffffff HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .await;
        let reply = text(&response);
        assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
        assert_eq!(body(&response), b"not found");
        assert!(!reply.contains("secret.txt"));

        let upload = exchange(
            addr,
            "PUT /r/ffffffffffffffffffffffffffffffff/0 HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .await;
        assert!(text(&upload).starts_with("HTTP/1.1 404"));

        serving.abort();
        std::fs::remove_dir_all(&root).ok();
    }
}
