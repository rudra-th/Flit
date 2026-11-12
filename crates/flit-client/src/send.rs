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
use futures_util::StreamExt;
use http_body_util::{StreamBody, combinators::BoxBody};
use hyper::body::{Bytes, Frame};
use hyper::header::{CONTENT_RANGE, HeaderValue};
use hyper::{Method, StatusCode};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::{Client, ClientError, Connection, RequestBody, SendProgress, bytes_body};

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
        self.send_files_with(targets, None).await
    }

    /// Send several files in one session, reporting live progress.
    ///
    /// `progress` is updated as bytes leave and as items get a result, so a UI
    /// can draw a bar without waiting for the session to close. Cancelling it
    /// stops the send at the next chunk boundary, tells the receiver to drop
    /// the session, and returns [`ClientError::Cancelled`].
    pub async fn send_files_with(
        &mut self,
        targets: &[SendTarget],
        progress: Option<&SendProgress>,
    ) -> Result<Vec<FileOutcome>, ClientError> {
        let progress = progress.cloned().unwrap_or_default();
        let mut outcomes: Vec<Option<FileOutcome>> = (0..targets.len()).map(|_| None).collect();
        let mut prepared: Vec<(usize, ManifestItem)> = Vec::new();

        for (index, target) in targets.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(ClientError::Cancelled);
            }
            match build_item(index, target, &progress).await {
                Ok(item) => prepared.push((index, item)),
                Err(err) => {
                    progress.item_done();
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

        progress.set_totals(
            prepared.iter().map(|(_, item)| item.size).sum(),
            prepared.len(),
        );

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

        let resume_offset =
            |item: &ManifestItem| session.resume.get(&item.id).copied().unwrap_or(0);

        // Bytes the receiver already holds count as sent, or a resumed send
        // would start its bar at zero while most of the data is already there.
        let already_held: u64 = prepared
            .iter()
            .filter(|(_, item)| accepted.contains(item.id.as_str()))
            .map(|(_, item)| resume_offset(item))
            .sum();
        progress.add_bytes(already_held);

        let mut failed = false;

        // Small files travel in pack requests: one round trip for a group of
        // them instead of one per file. A pack carries each item whole, so a
        // failed pack falls back to `PUT` and the normal resume path decides
        // whether the receiver already has the bytes.
        // Items the receiver already holds part of start from its offset. This
        // is how a re-run after a crash avoids sending bytes twice; without it
        // the first `PUT` would carry the whole file and be answered with a
        // `308`.
        let packable: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, (_, item))| {
                accepted.contains(item.id.as_str())
                    // A pack replaces an item whole, so partial items take `PUT`.
                    && resume_offset(item) == 0
                    && item.size <= PACK_THRESHOLD
            })
            .map(|(position, _)| position)
            .collect();

        // Positions that travelled in a pack request. Everything else takes
        // `PUT`, including small items excluded above because the receiver
        // already held part of them.
        let packed: HashSet<usize> = packable.iter().copied().collect();

        let mut start = 0;
        while start < packable.len() {
            if progress.is_cancelled() {
                self.cancel(&session.session_id).await;
                return Err(ClientError::Cancelled);
            }
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

            let sent = self
                .pack_upload(
                    &session.session_id,
                    &prepared,
                    &packable[start..end],
                    targets,
                    &progress,
                )
                .await;

            match sent {
                Ok(()) => {
                    for _ in &packable[start..end] {
                        progress.item_done();
                    }
                }
                Err(_) => {
                    for &position in &packable[start..end] {
                        let (index, item) = &prepared[position];
                        let outcome = self
                            .upload(
                                &session.session_id,
                                item,
                                &targets[*index].path,
                                resume_offset(item),
                                &progress,
                            )
                            .await;
                        progress.item_done();
                        if let Err(err) = outcome {
                            outcomes[*index] = Some(FileOutcome {
                                path: targets[*index].path.clone(),
                                name: item.name.clone(),
                                result: Err(err),
                            });
                            failed = true;
                            if progress.is_cancelled() {
                                self.cancel(&session.session_id).await;
                                return Err(ClientError::Cancelled);
                            }
                        }
                    }
                }
            }
            start = end;
        }

        for (index, item) in &prepared {
            if progress.is_cancelled() {
                self.cancel(&session.session_id).await;
                return Err(ClientError::Cancelled);
            }
            if !accepted.contains(item.id.as_str()) {
                let reason = session
                    .rejected
                    .iter()
                    .find(|rejected| rejected.id == item.id)
                    .map(|rejected| rejected.reason)
                    .unwrap_or(ErrorCode::BadRequest);
                progress.item_done();
                outcomes[*index] = Some(FileOutcome {
                    path: targets[*index].path.clone(),
                    name: item.name.clone(),
                    result: Err(ClientError::Rejected(reason)),
                });
                failed = true;
                continue;
            }
            if packed.contains(index) {
                // Already sent in a pack request, or its fallback failed and
                // recorded an outcome above.
                continue;
            }
            let outcome = self
                .upload(
                    &session.session_id,
                    item,
                    &targets[*index].path,
                    resume_offset(item),
                    &progress,
                )
                .await;
            progress.item_done();
            if let Err(err) = outcome {
                outcomes[*index] = Some(FileOutcome {
                    path: targets[*index].path.clone(),
                    name: item.name.clone(),
                    result: Err(err),
                });
                failed = true;
                if progress.is_cancelled() {
                    self.cancel(&session.session_id).await;
                    return Err(ClientError::Cancelled);
                }
            }
        }

        let ids: Vec<String> = prepared
            .iter()
            .filter(|(_, item)| accepted.contains(item.id.as_str()))
            .map(|(_, item)| item.id.clone())
            .collect();

        match self.finish(&session.session_id, &ids).await {
            Ok(results) => {
                // One lookup per item, not one scan per item: a 10,000 file
                // batch would otherwise compare every result with every item.
                let by_id: std::collections::HashMap<&str, &ItemResult> = results
                    .iter()
                    .map(|result| (result.id.as_str(), result))
                    .collect();
                for (index, item) in &prepared {
                    if outcomes[*index].is_some() {
                        continue;
                    }
                    let Some(result) = by_id.get(item.id.as_str()).copied() else {
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

    /// Stream the bytes from `offset`, following `308` offsets until the
    /// receiver has them all.
    async fn upload(
        &mut self,
        session_id: &str,
        item: &ManifestItem,
        path: &Path,
        offset: u64,
        progress: &SendProgress,
    ) -> Result<(), ClientError> {
        if offset >= item.size {
            // The receiver has the whole item, including the empty-file case.
            // `finish` still verifies and installs it.
            return Ok(());
        }
        let mut offset = offset;

        for _ in 0..MAX_RESUME_ATTEMPTS {
            // Bytes counted for **this attempt**, so a `308` can correct the
            // total: what the receiver kept is `offset`, not what was pushed.
            let attempt = Arc::new(AtomicU64::new(0));
            let body = file_body(path, offset, progress, Arc::clone(&attempt)).await?;
            let mut request = self.request(
                Method::PUT,
                format!("{API_PREFIX}/upload/{session_id}/{}", item.id),
                body,
            )?;
            if offset > 0 {
                // The body starts at `offset`, and the receiver will only
                // accept it there if the request says so. Without this header
                // it sees a body starting at zero, answers `308` again, and
                // the loop spins until it gives up.
                let value = HeaderValue::from_str(&format!("bytes {offset}-"))
                    .map_err(|e| ClientError::Decode(format!("bad resume offset: {e}")))?;
                request.headers_mut().insert(CONTENT_RANGE, value);
            }
            let timeout = self.transfer_timeout;
            let (status, headers, body) = self.execute_within(request, timeout).await?;

            match status {
                StatusCode::NO_CONTENT => return Ok(()),
                StatusCode::PERMANENT_REDIRECT => {
                    let next = headers
                        .get("x-flit-offset")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .ok_or_else(|| ClientError::Decode("308 without a usable offset".into()))?;
                    if next > item.size {
                        return Err(ClientError::Decode(format!(
                            "receiver claims {next} of {} bytes",
                            item.size
                        )));
                    }

                    // Make the progress bar agree with the receiver: bytes it
                    // did not keep were never sent, and bytes it already had
                    // are already done.
                    let counted = attempt.load(Ordering::Relaxed);
                    if next > counted {
                        progress.add_bytes(next - counted);
                    } else {
                        progress.subtract_bytes(counted - next);
                    }
                    offset = next;
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
        progress: &SendProgress,
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
        // Packs are one buffered body, so their bytes are counted when the
        // receiver has them. A UI sees per-pack granularity, not per-frame.
        // Only payload counts, not the framed header in front of it.
        let payload: u64 = positions
            .iter()
            .map(|&position| prepared[position].1.size)
            .sum();
        progress.add_bytes(payload);
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
    ///
    /// The common case is a connection that just died with the aborted body,
    /// so the first attempt failing is expected and a fresh connection is
    /// opened: the receiver still holds the session and still needs the
    /// `DELETE`, or its bytes sit there until the sweeper runs.
    async fn cancel(&mut self, session_id: &str) {
        match self.request(
            Method::DELETE,
            format!("{API_PREFIX}/upload/{session_id}"),
            bytes_body(Bytes::new()),
        ) {
            Ok(request) => match self.execute(request).await {
                Ok((status, _, _)) if status == StatusCode::NO_CONTENT => return,
                Ok((status, _, _)) => log::debug!("cancel answered {status}"),
                Err(err) => log::debug!("cancel on the send connection failed: {err}"),
            },
            Err(err) => log::debug!("could not build the cancel request: {err}"),
        }

        let addr = self.addr;
        let mut client = match Client::new(addr, &self.fingerprint) {
            Ok(client) => client,
            Err(err) => {
                log::debug!("could not rebuild a client to cancel: {err}");
                return;
            }
        };
        if let Some(pin) = &self.pin {
            client = client.with_pin(pin.clone());
        }
        client = client
            .with_timeout(self.control_timeout)
            .with_transfer_timeout(self.transfer_timeout);
        let mut fresh = match client.connect().await {
            Ok(fresh) => fresh,
            Err(err) => {
                log::debug!("could not reconnect to cancel: {err}");
                return;
            }
        };
        let request = fresh.request(
            Method::DELETE,
            format!("{API_PREFIX}/upload/{session_id}"),
            bytes_body(Bytes::new()),
        );
        if let Ok(request) = request {
            match fresh.execute(request).await {
                Ok((status, _, _)) if status == StatusCode::NO_CONTENT => {}
                Ok((status, _, _)) => log::debug!("cancel on a fresh connection answered {status}"),
                Err(err) => log::debug!("cancel on a fresh connection failed: {err}"),
            }
        }
        fresh.close();
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
async fn build_item(
    index: usize,
    target: &SendTarget,
    progress: &SendProgress,
) -> Result<ManifestItem, ClientError> {
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

    let sha256 = hash_file(path, progress).await?;
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
///
/// Hashing happens before the manifest exists, which for a large file is the
/// longest silent stretch of a send. It honours cancellation per chunk for that
/// reason: clicking stop should not wait for a 100 GiB file to finish hashing.
async fn hash_file(path: &Path, progress: &SendProgress) -> Result<String, ClientError> {
    let mut file = File::open(path)
        .await
        .map_err(|source| local_error(path, source))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];

    loop {
        if progress.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
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
///
/// Each chunk is counted into `progress` (and `attempt`) as it leaves, and a
/// cancelled send aborts the request at the next chunk instead of finishing a
/// file nobody is waiting for.
async fn file_body(
    path: &Path,
    offset: u64,
    progress: &SendProgress,
    attempt: Arc<AtomicU64>,
) -> Result<RequestBody, ClientError> {
    let mut file = File::open(path)
        .await
        .map_err(|source| local_error(path, source))?;
    if offset > 0 {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|source| local_error(path, source))?;
    }

    let counted = progress.clone();
    let stream = ReaderStream::with_capacity(file, 256 * 1024).map(
        move |chunk| -> Result<Frame<Bytes>, std::io::Error> {
            let chunk = chunk?;
            if counted.is_cancelled() {
                // Failing the body aborts the request; the caller sees the
                // cancel flag and reports it as a cancel, not a network error.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "send cancelled",
                ));
            }
            let len = chunk.len() as u64;
            counted.add_bytes(len);
            attempt.fetch_add(len, Ordering::Relaxed);
            Ok(Frame::data(chunk))
        },
    );
    Ok(BoxBody::new(StreamBody::new(stream)))
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
