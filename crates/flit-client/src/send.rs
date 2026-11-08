//! Sending one file: hash it, prepare a session, stream it, verify it.
//!
//! The order matters. The hash is computed before the manifest so the receiver
//! knows what to verify against before it writes anything, and the file is only
//! announced once its bytes can actually be read.
//!
//! Resumption is handled by the same loop as a first attempt: the receiver
//! answers `308` with the offset it already has, and the stream reopens from
//! there. A sender that cannot resume reports that instead of corrupting an
//! item.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use flit_api::{
    API_PREFIX, ErrorCode, FinishRequest, FinishResponse, ItemKind, ManifestItem, SessionResponse,
};
use futures_util::TryStreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Bytes, Frame};
use hyper::{Method, StatusCode};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{ClientError, Connection, RequestBody, bytes_body};

/// How many times a `308` resume may bounce before giving up.
const MAX_RESUME_ATTEMPTS: usize = 4;

/// Where a file ended up, as the receiver recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFile {
    /// Path relative to the receiver's destination root, after conflict
    /// resolution.
    pub saved_as: String,
    /// Bytes sent and verified.
    pub bytes: u64,
}

/// The sender's view of a finished transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// The receiver verified the hash and saved the file.
    Verified(SavedFile),
}

impl Connection {
    /// Send one local file.
    ///
    /// `name` overrides the name the receiver will see; the default is the
    /// local file name. Relative directories in `name` are sent as-is and are
    /// sanitized by the receiver.
    pub async fn send_file(
        &mut self,
        path: impl AsRef<Path>,
        name: Option<&str>,
    ) -> Result<SendOutcome, ClientError> {
        let path = path.as_ref();
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|source| local_error(path, source))?;
        if !metadata.is_file() {
            return Err(local_error(
                path,
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
            ));
        }

        let size = metadata.len();
        let remote_name = name
            .map(str::to_owned)
            .or_else(|| path.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .ok_or_else(|| {
                local_error(
                    path,
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "file has no name"),
                )
            })?;

        let sha256 = hash_file(path).await?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|age| age.as_secs() as i64);

        let item = ManifestItem {
            id: "i1".to_string(),
            name: remote_name,
            size,
            sha256,
            mtime,
            kind: ItemKind::File,
            text: None,
        };

        let session = self.prepare(&item).await?;
        let result = match self.upload(&session.session_id, &item, path).await {
            Ok(()) => self.finish(&session.session_id, &item.id, size).await,
            Err(err) => Err(err),
        };

        // A send that gives up must not leave half-written parts on the
        // receiver for an hour. Cancelling is best effort: the original error
        // is the one the caller needs to see.
        if result.is_err() {
            self.cancel(&session.session_id).await;
        }
        result
    }

    /// Announce one item and open a session.
    async fn prepare(&mut self, item: &ManifestItem) -> Result<SessionResponse, ClientError> {
        let mut manifest = serde_json::to_vec(item)
            .map_err(|e| ClientError::Decode(format!("could not encode the manifest: {e}")))?;
        manifest.push(b'\n');

        let request = self.request(
            Method::POST,
            format!("{API_PREFIX}/upload"),
            bytes_body(Bytes::from(manifest)),
        )?;
        let (status, _, body) = self.execute(request).await?;

        if status != StatusCode::CREATED {
            return Err(status_error(status, &body));
        }

        let session: SessionResponse = serde_json::from_slice(&body)
            .map_err(|e| ClientError::Decode(format!("bad prepare response: {e}")))?;
        if !session.accepted.iter().any(|id| id == &item.id) {
            let reason = session
                .rejected
                .iter()
                .find(|rejected| rejected.id == item.id)
                .map(|rejected| rejected.reason)
                .unwrap_or(ErrorCode::BadRequest);
            return Err(ClientError::Rejected(reason));
        }
        Ok(session)
    }

    /// Stream the bytes, following `308` offsets until the receiver has them all.
    async fn upload(
        &mut self,
        session_id: &str,
        item: &ManifestItem,
        path: &Path,
    ) -> Result<(), ClientError> {
        let mut offset = 0u64;

        for _ in 0..MAX_RESUME_ATTEMPTS {
            let body = file_body(path, offset).await?;
            let request = self.request(
                Method::PUT,
                format!("{API_PREFIX}/upload/{session_id}/{}", item.id),
                body,
            )?;
            let timeout = self.transfer_timeout;
            let (status, headers, body) = self.execute_within(request, timeout).await?;

            match status {
                StatusCode::NO_CONTENT => return Ok(()),
                StatusCode::PERMANENT_REDIRECT => {
                    offset = headers
                        .get("x-flit-offset")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .ok_or_else(|| ClientError::Decode("308 without a usable offset".into()))?;
                    if offset > item.size {
                        return Err(ClientError::Decode(format!(
                            "receiver claims {offset} of {} bytes",
                            item.size
                        )));
                    }
                }
                _ => return Err(status_error(status, &body)),
            }
        }

        Err(ClientError::Decode(format!(
            "receiver kept asking to resume; last offset {offset}"
        )))
    }

    /// Ask the receiver to verify and commit.
    async fn finish(
        &mut self,
        session_id: &str,
        item_id: &str,
        size: u64,
    ) -> Result<SendOutcome, ClientError> {
        let body = serde_json::to_vec(&FinishRequest {
            item_ids: vec![item_id.to_string()],
        })
        .map_err(|e| ClientError::Decode(format!("could not encode finish: {e}")))?;

        let request = self.request(
            Method::POST,
            format!("{API_PREFIX}/upload/{session_id}/finish"),
            bytes_body(Bytes::from(body)),
        )?;
        let (status, _, body) = self.execute(request).await?;
        if status != StatusCode::OK {
            return Err(status_error(status, &body));
        }

        let response: FinishResponse = serde_json::from_slice(&body)
            .map_err(|e| ClientError::Decode(format!("bad finish response: {e}")))?;
        let result = response
            .items
            .iter()
            .find(|item| item.id == item_id)
            .ok_or_else(|| ClientError::Decode("receiver forgot the item".into()))?;

        if !result.verified {
            return Err(ClientError::NotVerified);
        }
        Ok(SendOutcome::Verified(SavedFile {
            saved_as: result
                .saved_as
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            bytes: size,
        }))
    }

    /// Tell the receiver to drop a session we are abandoning.
    ///
    /// Best effort by design: this runs on the error path, and a failure to
    /// clean up must never replace the error that caused it.
    async fn cancel(&mut self, session_id: &str) {
        let request = match self.request(
            Method::DELETE,
            format!("{API_PREFIX}/upload/{session_id}"),
            bytes_body(Bytes::new()),
        ) {
            Ok(request) => request,
            Err(err) => {
                log::debug!("could not build the cancel request: {err}");
                return;
            }
        };
        match self.execute(request).await {
            Ok((status, _, _)) if status == StatusCode::NO_CONTENT => {}
            Ok((status, _, _)) => log::debug!("cancel answered {status}"),
            Err(err) => log::debug!("cancel failed: {err}"),
        }
    }
}

/// SHA-256 of a file, streamed so a 4 GiB file does not need 4 GiB of memory.
async fn hash_file(path: &Path) -> Result<String, ClientError> {
    let mut file = File::open(path)
        .await
        .map_err(|source| local_error(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| local_error(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// A body that streams `path` from `offset` to the end.
async fn file_body(path: &Path, offset: u64) -> Result<RequestBody, ClientError> {
    let mut file = File::open(path)
        .await
        .map_err(|source| local_error(path, source))?;
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|source| local_error(path, source))?;
    }

    let stream = ReaderStream::with_capacity(file, 256 * 1024).map_ok(Frame::data);
    Ok(StreamBody::new(stream).boxed())
}

/// Wrap a local I/O error with the path that caused it.
fn local_error(path: &Path, source: std::io::Error) -> ClientError {
    ClientError::Local {
        path: PathBuf::from(path),
        source,
    }
}

/// Turn a non-success response into the best error we can describe.
fn status_error(status: StatusCode, body: &Bytes) -> ClientError {
    if let Ok(error) = serde_json::from_slice::<flit_api::ApiErrorBody>(body) {
        if error.code == ErrorCode::Internal {
            return ClientError::Status {
                status: status.as_u16(),
                body: error.message,
            };
        }
        return ClientError::Status {
            status: status.as_u16(),
            body: format!("{}: {}", error.code, error.message),
        };
    }
    ClientError::Status {
        status: status.as_u16(),
        body: String::from_utf8_lossy(body).into_owned(),
    }
}
