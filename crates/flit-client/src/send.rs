//! Sending files: hash them, prepare a session, stream them, verify them.
//!
//! One [`SendTarget`] is a single-file send; several share one session, so the
//! receiver sees a batch and can accept or refuse items individually. The order
//! matters. Every hash is computed before the manifest so the receiver knows
//! what to verify against before it writes anything, and a file is only
//! announced once its bytes can actually be read.
//!
//! Resumption is handled by the same loop as a first attempt: the receiver
//! answers `308` with the offset it already has, and the stream reopens from
//! there. A sender that cannot resume reports that instead of corrupting an
//! item.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use flit_api::{
    API_PREFIX, ErrorCode, FinishRequest, FinishResponse, ItemKind, ItemResult, ManifestItem,
    PACK_THRESHOLD, SessionResponse,
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

/// Payload bytes per pack request.
///
/// Bounds what one batch holds in memory while still turning hundreds of small
/// files into one request instead of hundreds.
const PACK_REQUEST_BYTES: u64 = 8 * 1024 * 1024;

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

/// One local file to send and the name the receiver should see.
#[derive(Debug, Clone)]
pub struct SendTarget {
    /// File to read.
    pub path: PathBuf,
    /// Name to offer; `None` uses the local file name.
    pub name: Option<String>,
}

/// One file's result inside [`Connection::send_files`].
#[derive(Debug)]
pub struct FileOutcome {
    /// Local path this result belongs to.
    pub path: PathBuf,
    /// Name that was offered to the receiver.
    pub name: String,
    /// Where it landed and how many bytes arrived, or why this file failed.
    pub result: Result<SavedFile, ClientError>,
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
        let target = SendTarget {
            path: path.as_ref().to_path_buf(),
            name: name.map(str::to_owned),
        };
        let mut outcomes = self.send_files(std::slice::from_ref(&target)).await?;
        let outcome = outcomes
            .pop()
            .ok_or_else(|| ClientError::Decode("no outcome for a single-file send".into()))?;
        outcome.result.map(SendOutcome::Verified)
    }

    /// Send several files in **one session**, hashing them before the manifest
    /// is announced.
    ///
    /// The session is the unit of rejection and cancellation: a receiver can
    /// accept some items and refuse others, and this method reports each file
    /// separately. Files that were already verified stay installed even when a
    /// later item fails, because the receiver links each one into place at
    /// finish time.
    ///
    /// An error here means the session itself could not be created, uploaded
    /// to, or finished; per-file failures are inside the returned list.
    pub async fn send_files(
        &mut self,
        targets: &[SendTarget],
    ) -> Result<Vec<FileOutcome>, ClientError> {
        let mut outcomes: Vec<Option<FileOutcome>> = (0..targets.len()).map(|_| None).collect();
        let mut prepared: Vec<(usize, ManifestItem)> = Vec::new();

        for (index, target) in targets.iter().enumerate() {
            match build_item(index, target).await {
                Ok(item) => prepared.push((index, item)),
                Err(err) => {
                    outcomes[index] = Some(FileOutcome {
                        path: target.path.clone(),
                        name: offered_name(target),
                        result: Err(err),
                    });
                }
            }
        }

        if prepared.is_empty() {
            return Ok(finish_outcomes(outcomes));
        }

        let session = {
            let items: Vec<ManifestItem> = prepared.iter().map(|(_, item)| item.clone()).collect();
            self.prepare(&items).await?
        };
        let accepted: HashSet<&str> = session.accepted.iter().map(String::as_str).collect();
        if accepted.is_empty() {
            let reason = session
                .rejected
                .first()
                .map(|rejected| rejected.reason)
                .unwrap_or(ErrorCode::BadRequest);
            return Err(ClientError::Rejected(reason));
        }

        let mut failed = false;

        // Small files travel in pack requests: one round trip for a group of
        // them instead of one per file. A pack carries each item whole, so a
        // failed pack falls back to `PUT` and the normal resume path decides
        // whether the receiver already has the bytes.
        let packable: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, (_, item))| {
                accepted.contains(item.id.as_str()) && item.size <= PACK_THRESHOLD
            })
            .map(|(position, _)| position)
            .collect();

        let mut start = 0;
        while start < packable.len() {
            let mut end = start;
            let mut bytes = 0u64;
            while end < packable.len() {
                let size = prepared[packable[end]].1.size;
                if end > start && bytes + size > PACK_REQUEST_BYTES {
                    break;
                }
                bytes += size;
                end += 1;
            }

            if self
                .pack_upload(&session.session_id, &prepared, &packable[start..end], targets)
                .await
                .is_err()
            {
                for &position in &packable[start..end] {
                    let (index, item) = &prepared[position];
                    if let Err(err) = self
                        .upload(&session.session_id, item, &targets[*index].path)
                        .await
                    {
                        outcomes[*index] = Some(FileOutcome {
                            path: targets[*index].path.clone(),
                            name: item.name.clone(),
                            result: Err(err),
                        });
                        failed = true;
                    }
                }
            }
            start = end;
        }

        for (index, item) in &prepared {
            if !accepted.contains(item.id.as_str()) {
                let reason = session
                    .rejected
                    .iter()
                    .find(|rejected| rejected.id == item.id)
                    .map(|rejected| rejected.reason)
                    .unwrap_or(ErrorCode::BadRequest);
                outcomes[*index] = Some(FileOutcome {
                    path: targets[*index].path.clone(),
                    name: item.name.clone(),
                    result: Err(ClientError::Rejected(reason)),
                });
                failed = true;
                continue;
            }
            if item.size <= PACK_THRESHOLD {
                // Already sent in a pack request, or its fallback failed and
                // recorded an outcome above.
                continue;
            }
            if let Err(err) = self
                .upload(&session.session_id, item, &targets[*index].path)
                .await
            {
                outcomes[*index] = Some(FileOutcome {
                    path: targets[*index].path.clone(),
                    name: item.name.clone(),
                    result: Err(err),
                });
                failed = true;
            }
        }

        let ids: Vec<String> = prepared
            .iter()
            .filter(|(_, item)| accepted.contains(item.id.as_str()))
            .map(|(_, item)| item.id.clone())
            .collect();

        match self.finish(&session.session_id, &ids).await {
            Ok(results) => {
                for (index, item) in &prepared {
                    if outcomes[*index].is_some() {
                        continue;
                    }
                    let Some(result) = results.iter().find(|result| result.id == item.id) else {
                        outcomes[*index] = Some(FileOutcome {
                            path: targets[*index].path.clone(),
                            name: item.name.clone(),
                            result: Err(ClientError::Decode(
                                "receiver forgot an item it accepted".into(),
                            )),
                        });
                        failed = true;
                        continue;
                    };
                    if result.verified {
                        outcomes[*index] = Some(FileOutcome {
                            path: targets[*index].path.clone(),
                            name: item.name.clone(),
                            result: Ok(SavedFile {
                                saved_as: result
                                    .saved_as
                                    .clone()
                                    .unwrap_or_else(|| "unknown".to_string()),
                                bytes: item.size,
                            }),
                        });
                    } else {
                        outcomes[*index] = Some(FileOutcome {
                            path: targets[*index].path.clone(),
                            name: item.name.clone(),
                            result: Err(ClientError::NotVerified),
                        });
                        failed = true;
                    }
                }
            }
            Err(err) => {
                self.cancel(&session.session_id).await;
                return Err(err);
            }
        }

        // A partially failed session must not leave parts behind. Files that
        // were already verified are already linked into place and survive.
        if failed {
            self.cancel(&session.session_id).await;
        }

        Ok(finish_outcomes(outcomes))
    }

    /// Announce a batch of items and open one session.
    async fn prepare(&mut self, items: &[ManifestItem]) -> Result<SessionResponse, ClientError> {
        let mut manifest = Vec::new();
        for item in items {
            serde_json::to_writer(&mut manifest, item)
                .map_err(|e| ClientError::Decode(format!("could not encode the manifest: {e}")))?;
            manifest.push(b'\n');
        }

        let request = self.request(
            Method::POST,
            format!("{API_PREFIX}/upload"),
            bytes_body(Bytes::from(manifest)),
        )?;
        let (status, _, body) = self.execute(request).await?;

        if status != StatusCode::CREATED {
            return Err(status_error(status, &body));
        }

        serde_json::from_slice(&body)
            .map_err(|e| ClientError::Decode(format!("bad prepare response: {e}")))
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

    /// Send a group of small files in one `pack` request.
    ///
    /// The body is `u32 length | JSON header | payload` per item, exactly as
    /// §6.5 defines it. Callers treat any error as "fall back to `PUT`", so the
    /// reason is not inspected here.
    async fn pack_upload(
        &mut self,
        session_id: &str,
        prepared: &[(usize, ManifestItem)],
        positions: &[usize],
        targets: &[SendTarget],
    ) -> Result<(), ClientError> {
        let mut body = Vec::new();
        for &position in positions {
            let (index, item) = &prepared[position];
            let path = &targets[*index].path;
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|source| local_error(path, source))?;
            if bytes.len() as u64 != item.size {
                return Err(local_error(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "file changed size while sending",
                    ),
                ));
            }
            let header = serde_json::json!({
                "id": &item.id,
                "name": &item.name,
                "size": item.size,
                "sha256": &item.sha256,
            });
            let header = serde_json::to_vec(&header)
                .map_err(|e| ClientError::Decode(format!("could not encode a pack header: {e}")))?;
            if header.len() > u32::MAX as usize {
                return Err(ClientError::Decode("pack header is too large".into()));
            }
            body.extend_from_slice(&(header.len() as u32).to_le_bytes());
            body.extend_from_slice(&header);
            body.extend_from_slice(&bytes);
        }

        let request = self.request(
            Method::POST,
            format!("{API_PREFIX}/upload/{session_id}/pack"),
            bytes_body(Bytes::from(body)),
        )?;
        let timeout = self.transfer_timeout;
        let (status, _, response) = self.execute_within(request, timeout).await?;
        if status != StatusCode::NO_CONTENT {
            return Err(status_error(status, &response));
        }
        Ok(())
    }

    /// Ask the receiver to verify and commit the named items.
    async fn finish(
        &mut self,
        session_id: &str,
        ids: &[String],
    ) -> Result<Vec<ItemResult>, ClientError> {
        let body = serde_json::to_vec(&FinishRequest {
            item_ids: ids.to_vec(),
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
        Ok(response.items)
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

/// Name offered to the receiver: the override, else the local file name.
fn offered_name(target: &SendTarget) -> String {
    target
        .name
        .clone()
        .or_else(|| {
            target
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| target.path.display().to_string())
}

/// Hash and describe one file, with its id derived from its place in the batch.
async fn build_item(index: usize, target: &SendTarget) -> Result<ManifestItem, ClientError> {
    let path = &target.path;
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|source| local_error(path, source))?;
    if !metadata.is_file() {
        return Err(local_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        ));
    }
    if offered_name(target).is_empty() {
        return Err(local_error(
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "file has no name"),
        ));
    }

    let sha256 = hash_file(path).await?;
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs() as i64);

    Ok(ManifestItem {
        id: format!("i{}", index + 1),
        name: offered_name(target),
        size: metadata.len(),
        sha256,
        mtime,
        kind: ItemKind::File,
        text: None,
    })
}

/// Turn the slot vector into the caller-facing order, filling impossible gaps.
fn finish_outcomes(outcomes: Vec<Option<FileOutcome>>) -> Vec<FileOutcome> {
    outcomes
        .into_iter()
        .map(|outcome| {
            outcome.unwrap_or_else(|| FileOutcome {
                path: PathBuf::new(),
                name: String::new(),
                result: Err(ClientError::Decode("no result was recorded".into())),
            })
        })
        .collect()
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
