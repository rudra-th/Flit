//! HTTP routing and the upload endpoints.
//!
//! Kept separate from `lib.rs` so the transport setup and the protocol logic can
//! be read independently.

use std::{convert::Infallible, io::Write, net::IpAddr, sync::Arc};

use flit_api::{
    API_PREFIX, ApiErrorBody, ErrorCode, FinishRequest, Info, MAX_ITEMS, MAX_MANIFEST_BYTES,
    ManifestItem, PIN_HEADER, PROTO_HEADER, PROTO_VERSION, PROTO_VERSION_STR,
};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{self, HeaderValue},
};
use sha2::Digest;

use crate::{pin::PinGuard, session::Sessions};

/// Every response body, boxed so handlers can differ in shape.
pub type ResponseBody = BoxBody<Bytes, Infallible>;

/// What a handler needs that is not the request.
pub struct State {
    /// Advertised identity and capabilities.
    pub info: Info,
    /// Live sessions and the destination root.
    pub sessions: Sessions,
    /// PIN policy and recent failures.
    pub pins: PinGuard,
}

/// Route one request. Unknown shapes never reach the filesystem.
///
/// The peer address is used only for PIN throttling; it never grants access to
/// anything, because the TLS certificate and the PIN are the whole policy.
pub async fn route(
    request: Request<Incoming>,
    state: Arc<State>,
    peer: IpAddr,
) -> Response<ResponseBody> {
    if let Some(version) = request.headers().get(PROTO_HEADER) {
        let version = version.to_str().unwrap_or("<invalid>");
        if version != PROTO_VERSION_STR {
            return error(
                ErrorCode::Unsupported,
                format!("this device speaks protocol {PROTO_VERSION}, not {version}"),
            );
        }
    }

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

    // `/info` is deliberately open: it is what discovery and pinning are built
    // on, and a peer cannot send a PIN it has not been told about yet. Every
    // other endpoint, including `DELETE`, needs the PIN when one is configured.
    let open = method == Method::GET && path == format!("{API_PREFIX}/info");
    if !open {
        let presented = request
            .headers()
            .get(PIN_HEADER)
            .and_then(|value| value.to_str().ok());
        if let Err(code) = state.pins.check(peer, presented) {
            return error(code, pin_message(code));
        }
    }

    match (method, segments.as_slice()) {
        (Method::GET, ["api", "v1", "info"]) => json(&state.info),
        (Method::POST, ["api", "v1", "upload"]) => prepare(request, &state).await,
        (Method::PUT, ["api", "v1", "upload", session, item]) => {
            upload(request, &state, session, item).await
        }
        (Method::POST, ["api", "v1", "upload", session, "finish"]) => {
            finish(request, &state, session).await
        }
        (Method::DELETE, ["api", "v1", "upload", session]) => cancel(&state, session).await,
        _ => error(
            ErrorCode::NotFound,
            format!("no such endpoint: {} {path}", request.method()),
        ),
    }
}

/// `POST /api/v1/upload`: streamed NDJSON manifest, one item per line.
async fn prepare(request: Request<Incoming>, state: &Arc<State>) -> Response<ResponseBody> {
    let items = match read_manifest(request.into_body()).await {
        Ok(items) => items,
        Err(code) => return error(code, "manifest could not be read"),
    };

    match state.sessions.begin(items).await {
        Ok((_session, response)) => json_with_status(StatusCode::CREATED, &response),
        Err(code) => error(code, "manifest was refused"),
    }
}

/// `PUT /api/v1/upload/{session}/{item}`: raw bytes, optional resume.
async fn upload(
    request: Request<Incoming>,
    state: &Arc<State>,
    session_id: &str,
    item_id: &str,
) -> Response<ResponseBody> {
    let Some(session) = state.sessions.get(session_id).await else {
        return error(ErrorCode::NotFound, "no such session");
    };
    let mut session = session.lock().await;
    if session.cancelled {
        return error(ErrorCode::NotFound, "session was cancelled");
    }

    let (parts, body) = request.into_parts();
    let start = match content_range_start(parts.headers.get(header::CONTENT_RANGE)) {
        Ok(start) => start,
        Err(code) => return error(code, "Content-Range must look like `bytes 1048576-`"),
    };

    let Some(item) = session.items.get_mut(item_id) else {
        return error(ErrorCode::NotFound, format!("no such item: {item_id}"));
    };
    if item.done {
        return error(ErrorCode::Conflict, "item is already committed");
    }
    if start > item.received {
        return error(
            ErrorCode::Conflict,
            format!(
                "asked to resume at {start}, but only {} bytes are committed",
                item.received
            ),
        );
    }
    if start < item.received {
        return offset_response(item.received);
    }

    let remaining = item.size - item.received;
    let mut file = match open_part(item, remaining) {
        Ok(file) => file,
        Err(err) => return io_error("could not open the part file", err),
    };

    // The body may be longer than the declared size. Anything past the declared
    // length is refused rather than written, so a lying manifest cannot fill the
    // disk.
    let mut written = 0u64;
    let mut too_much = false;
    let mut body = body;

    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(err) => {
                log::debug!("upload body ended early: {err}");
                break;
            }
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if data.is_empty() {
            continue;
        }
        let room = remaining - written;
        let take = usize::try_from(room.min(data.len() as u64)).unwrap_or(data.len());
        if take > 0 {
            if let Err(err) = file.write_all(&data[..take]) {
                return io_error("could not write to disk", err);
            }
            item.hasher.update(&data[..take]);
            item.received += take as u64;
            written += take as u64;
        }
        if take < data.len() {
            too_much = true;
            break;
        }
    }

    if let Err(err) = file.flush() {
        return io_error("could not flush to disk", err);
    }
    if too_much {
        return error(
            ErrorCode::TooLarge,
            "body was longer than the size declared in the manifest",
        );
    }
    if written < remaining {
        return error(
            ErrorCode::BadRequest,
            format!("body ended after {written} of {remaining} bytes; resume with Content-Range"),
        );
    }

    empty(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/upload/{session}/finish`: verify, resolve collisions, commit.
async fn finish(
    request: Request<Incoming>,
    state: &Arc<State>,
    session_id: &str,
) -> Response<ResponseBody> {
    let request: FinishRequest = match read_json(request.into_body()).await {
        Ok(body) => body,
        Err(code) => return error(code, "finish body must be JSON"),
    };

    let Some(session) = state.sessions.get(session_id).await else {
        return error(ErrorCode::NotFound, "no such session");
    };

    let response = {
        let mut guard = session.lock().await;
        if guard.cancelled {
            return error(ErrorCode::NotFound, "session was cancelled");
        }
        state.sessions.finish(&mut guard, &request.item_ids)
    };

    // A session with anything unverified stays open: the sender may re-send the
    // bytes and finish again. A fully verified session is done.
    if response.items.iter().all(|item| item.verified) {
        state.sessions.take(session_id).await;
    }

    json(&response)
}

/// `DELETE /api/v1/upload/{session}`: stop and remove `.part` files.
async fn cancel(state: &Arc<State>, session_id: &str) -> Response<ResponseBody> {
    match state.sessions.take(session_id).await {
        Some(_) => empty(StatusCode::NO_CONTENT),
        None => error(ErrorCode::NotFound, "no such session"),
    }
}

/// The words behind a PIN failure.
///
/// "Wrong" and "locked out" share one message on purpose: a distinct message
/// would tell an attacker how many guesses are left in the window.
fn pin_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::PinRequired => "this device needs a PIN for first contact",
        _ => "the PIN did not match, or too many attempts were made",
    }
}

/// Read a streamed NDJSON manifest without ever holding it twice.
async fn read_manifest(mut body: Incoming) -> Result<Vec<ManifestItem>, ErrorCode> {
    let mut pending: Vec<u8> = Vec::new();
    let mut items = Vec::new();
    let mut total = 0usize;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| ErrorCode::BadRequest)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        total += data.len();
        if total > MAX_MANIFEST_BYTES {
            return Err(ErrorCode::TooLarge);
        }
        pending.extend_from_slice(&data);

        while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=index).collect();
            push_line(&line[..line.len() - 1], &mut items)?;
            if items.len() > MAX_ITEMS {
                return Err(ErrorCode::TooLarge);
            }
        }
    }

    if !pending.is_empty() {
        push_line(&pending, &mut items)?;
    }
    if items.len() > MAX_ITEMS {
        return Err(ErrorCode::TooLarge);
    }
    Ok(items)
}

/// Parse one manifest line.
fn push_line(line: &[u8], items: &mut Vec<ManifestItem>) -> Result<(), ErrorCode> {
    let text = std::str::from_utf8(line).map_err(|_| ErrorCode::BadRequest)?;
    if text.trim().is_empty() {
        return Ok(());
    }
    let item: ManifestItem = serde_json::from_str(text).map_err(|_| ErrorCode::BadRequest)?;
    items.push(item);
    Ok(())
}

/// Read a small JSON body. Finish requests are tiny, so the cap is generous but
/// not unbounded.
async fn read_json<T: serde::de::DeserializeOwned>(body: Incoming) -> Result<T, ErrorCode> {
    const MAX_BODY: usize = 64 * 1024;

    let bytes = body
        .collect()
        .await
        .map_err(|_| ErrorCode::BadRequest)?
        .to_bytes();
    if bytes.len() > MAX_BODY {
        return Err(ErrorCode::TooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| ErrorCode::BadRequest)
}

/// Open the part file for appending, creating it when this is a fresh item.
fn open_part(item: &crate::session::Item, remaining: u64) -> std::io::Result<std::fs::File> {
    if item.received == 0 {
        std::fs::File::create(&item.part)
    } else {
        debug_assert!(remaining > 0 || item.size == item.received);
        std::fs::OpenOptions::new().append(true).open(&item.part)
    }
}

/// Start offset from a `Content-Range` header, defaulting to zero.
fn content_range_start(value: Option<&HeaderValue>) -> Result<u64, ErrorCode> {
    let Some(value) = value else {
        return Ok(0);
    };
    let text = value.to_str().map_err(|_| ErrorCode::BadRequest)?;
    let rest = text.strip_prefix("bytes ").ok_or(ErrorCode::BadRequest)?;
    let start = rest.split('-').next().ok_or(ErrorCode::BadRequest)?;
    start
        .trim()
        .parse::<u64>()
        .map_err(|_| ErrorCode::BadRequest)
}

/// 308 with `X-Flit-Offset`, telling the sender where to resume.
fn offset_response(committed: u64) -> Response<ResponseBody> {
    let mut response = empty(StatusCode::PERMANENT_REDIRECT);
    if let Ok(value) = HeaderValue::from_str(&committed.to_string()) {
        response.headers_mut().insert("x-flit-offset", value);
    }
    response
}

/// JSON response with 200.
fn json<T: serde::Serialize>(value: &T) -> Response<ResponseBody> {
    json_with_status(StatusCode::OK, value)
}

/// JSON response with an explicit status.
fn json_with_status<T: serde::Serialize>(status: StatusCode, value: &T) -> Response<ResponseBody> {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(full(Bytes::from(body)));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            add_proto_header(&mut response);
            response
        }
        Err(err) => error(
            ErrorCode::Internal,
            format!("could not serialize response: {err}"),
        ),
    }
}

/// The uniform error body from `docs/protocol.md` §5.1.
pub fn error(code: ErrorCode, message: impl Into<String>) -> Response<ResponseBody> {
    let body = ApiErrorBody::new(code, message);
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    let mut response = Response::new(full(Bytes::from(bytes)));
    *response.status_mut() = status_for(code);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    add_proto_header(&mut response);
    response
}

/// Map an I/O failure to the closest protocol error.
fn io_error(context: &str, err: std::io::Error) -> Response<ResponseBody> {
    let code = match err.raw_os_error() {
        // ENOSPC on Unix, ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL on Windows.
        Some(28) | Some(112) | Some(39) => ErrorCode::DiskFull,
        _ => ErrorCode::Internal,
    };
    log::warn!("{context}: {err}");
    error(code, format!("{context}: {err}"))
}

/// Status code for a protocol error, per §5.1.
fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::PinRequired | ErrorCode::PinIncorrect => StatusCode::FORBIDDEN,
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Conflict | ErrorCode::Cancelled => StatusCode::CONFLICT,
        ErrorCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Unsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ErrorCode::DiskFull => StatusCode::INSUFFICIENT_STORAGE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Empty-body response with an explicit status.
fn empty(status: StatusCode) -> Response<ResponseBody> {
    let mut response = Response::new(empty_body());
    *response.status_mut() = status;
    add_proto_header(&mut response);
    response
}

/// Wrap bytes in the boxed body type.
fn full(bytes: Bytes) -> ResponseBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

/// An empty boxed body.
fn empty_body() -> ResponseBody {
    full(Bytes::new())
}

/// Echo the protocol version on every response.
fn add_proto_header(response: &mut Response<ResponseBody>) {
    response
        .headers_mut()
        .insert(PROTO_HEADER, HeaderValue::from_static(PROTO_VERSION_STR));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_is_parsed_or_refused() {
        assert_eq!(content_range_start(None), Ok(0));
        let open = HeaderValue::from_static("bytes 1048576-");
        assert_eq!(content_range_start(Some(&open)), Ok(1_048_576));
        let closed = HeaderValue::from_static("bytes 1048576-5242879/5242880");
        assert_eq!(content_range_start(Some(&closed)), Ok(1_048_576));
        for bad in ["1048576-", "bytes -", "bytes abc-", "items 1-"] {
            let value = HeaderValue::from_str(bad).unwrap();
            assert_eq!(
                content_range_start(Some(&value)),
                Err(ErrorCode::BadRequest),
                "{bad}"
            );
        }
    }

    #[test]
    fn status_codes_follow_the_specification() {
        assert_eq!(status_for(ErrorCode::PinIncorrect), StatusCode::FORBIDDEN);
        assert_eq!(
            status_for(ErrorCode::TooLarge),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            status_for(ErrorCode::DiskFull),
            StatusCode::INSUFFICIENT_STORAGE
        );
        assert_eq!(status_for(ErrorCode::NotFound), StatusCode::NOT_FOUND);
    }
}
