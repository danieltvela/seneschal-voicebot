//! Resumable, checksum-verified HTTP downloads (issue #35).
//!
//! Files download to `<final>.part` with HTTP `Range` resume; the digest is
//! computed while streaming and the file is only renamed to its final location
//! after the SHA-256 matches the catalog entry. A mismatch removes the partial
//! file so a retry starts clean.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::catalog::ModelFile;

/// Progress callback: `(file_rel_path, downloaded_in_file, total_in_file)`.
pub type OnProgress<'a> = &'a (dyn Fn(&str, u64, u64) + Send + Sync);

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("HTTP {status} from {url}")]
    Http { url: String, status: u16 },
    #[error("checksum mismatch for {path}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("download cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Long-lived HTTP client for downloads: no total timeout (large files), but a
/// bounded connect timeout so a dead host fails fast.
pub fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build download HTTP client")
}

/// `<final>.part` companion path for a target file.
pub fn part_file(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_os_string();
    os.push(".part");
    PathBuf::from(os)
}

/// Stream a file through SHA-256 and compare with `expected`.
pub async fn verify_file(path: &Path, expected: &str) -> Result<bool, DownloadError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()) == expected)
}

/// Download one catalog file into `root`, resuming from an existing `.part`
/// and verifying the catalog SHA-256 before the final rename.
pub async fn download_file(
    client: &reqwest::Client,
    root: &Path,
    model_file: &ModelFile,
    cancel: &CancellationToken,
    on_progress: OnProgress<'_>,
) -> Result<(), DownloadError> {
    let final_path = root.join(&model_file.rel_path);
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Already present and valid? A corrupt legacy file is replaced.
    if final_path.is_file() {
        match &model_file.sha256 {
            Some(expected) => {
                if verify_file(&final_path, expected).await? {
                    on_progress(
                        &model_file.rel_path,
                        model_file.size_bytes,
                        model_file.size_bytes,
                    );
                    return Ok(());
                }
                tokio::fs::remove_file(&final_path).await?;
            }
            None => return Ok(()),
        }
    }

    let part_path = part_file(&final_path);
    let mut hasher = Sha256::new();
    let mut start: u64 = 0;
    if part_path.is_file() {
        // Hash the existing partial bytes so the resumed download can be
        // verified as a whole.
        let mut part = tokio::fs::File::open(&part_path).await?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = part.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            start += read as u64;
        }
    }

    let mut request = client.get(&model_file.url);
    if start > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={start}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| anyhow::anyhow!("request to {} failed: {error}", model_file.url))?;
    let status = response.status();
    let resumed = start > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !status.is_success() {
        return Err(DownloadError::Http {
            url: model_file.url.clone(),
            status: status.as_u16(),
        });
    }

    let remaining = response.content_length().unwrap_or(0);
    let total = if resumed {
        let total = start + remaining;
        if total > 0 {
            total
        } else {
            model_file.size_bytes
        }
    } else {
        // Server ignored the Range header (or the part was rejected): restart.
        hasher = Sha256::new();
        start = 0;
        if remaining > 0 {
            remaining
        } else {
            model_file.size_bytes
        }
    };

    // Bail out before touching the part file when cancellation raced the
    // response: the partial bytes stay resumable.
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    let mut target = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resumed)
        .append(resumed)
        .open(&part_path)
        .await?;

    let mut stream = response.bytes_stream();
    let mut downloaded = start;
    loop {
        let chunk = tokio::select! {
            // Biased: a cancelled token wins over an in-flight chunk, so
            // cancellation is deterministic even on fast bodies.
            biased;
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            chunk = stream.next() => match chunk {
                Some(chunk) => chunk,
                None => break,
            },
        };
        let chunk = chunk
            .map_err(|error| anyhow::anyhow!("stream from {} failed: {error}", model_file.url))?;
        target.write_all(&chunk).await?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        on_progress(&model_file.rel_path, downloaded, total);
    }
    target.flush().await?;

    let actual = hex::encode(hasher.finalize());
    if let Some(expected) = &model_file.sha256
        && actual != *expected
    {
        // A mismatched partial is useless for resume: drop it.
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(DownloadError::ChecksumMismatch {
            path: model_file.rel_path.clone(),
            expected: expected.clone(),
            actual,
        });
    }

    tokio::fs::rename(&part_path, &final_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PAYLOAD: &[u8] = b"hello seneschal models";

    fn model_file(url: &str) -> ModelFile {
        ModelFile {
            rel_path: "test-model.bin".to_string(),
            url: url.to_string(),
            size_bytes: PAYLOAD.len() as u64,
            sha256: Some(hex::encode(Sha256::digest(PAYLOAD))),
        }
    }

    async fn serve_full() -> (MockServer, String) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(PAYLOAD))
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        (server, url)
    }

    #[tokio::test]
    async fn downloads_verify_and_rename_to_final() {
        let dir = tempfile::TempDir::new().unwrap();
        let (server, url) = serve_full().await;
        let cancel = CancellationToken::new();

        download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();

        let final_path = dir.path().join("test-model.bin");
        assert!(final_path.is_file());
        assert!(!part_file(&final_path).exists());
        assert_eq!(tokio::fs::read(&final_path).await.unwrap(), PAYLOAD);
        let _ = server;
    }

    #[tokio::test]
    async fn checksum_mismatch_leaves_no_final_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"corrupt"))
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let cancel = CancellationToken::new();

        let error = download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(error, DownloadError::ChecksumMismatch { .. }));
        assert!(!dir.path().join("test-model.bin").exists());
        assert!(!part_file(&dir.path().join("test-model.bin")).exists());
    }

    #[tokio::test]
    async fn resumes_from_partial_with_range_request() {
        let dir = tempfile::TempDir::new().unwrap();
        let server = MockServer::start().await;
        let final_path = dir.path().join("test-model.bin");
        let part_path = part_file(&final_path);

        // Simulate an interrupted download: first 5 bytes on disk.
        tokio::fs::write(&part_path, &PAYLOAD[..5]).await.unwrap();

        let saw_range = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_range_clone = std::sync::Arc::clone(&saw_range);
        Mock::given(method("GET"))
            .and(path("/model.bin"))
            .respond_with(move |request: &wiremock::Request| {
                if request
                    .headers
                    .get("range")
                    .is_some_and(|value| value.to_str().unwrap_or("").starts_with("bytes=5-"))
                {
                    saw_range_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    ResponseTemplate::new(206).set_body_bytes(PAYLOAD[5..].to_vec())
                } else {
                    ResponseTemplate::new(200).set_body_bytes(PAYLOAD)
                }
            })
            .mount(&server)
            .await;
        let url = format!("{}/model.bin", server.uri());
        let cancel = CancellationToken::new();

        download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();

        assert!(saw_range.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(tokio::fs::read(&final_path).await.unwrap(), PAYLOAD);
        assert!(!part_path.exists());
    }

    #[tokio::test]
    async fn restarts_cleanly_when_server_ignores_range() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_server, url) = serve_full().await;
        let cancel = CancellationToken::new();
        let part_path = part_file(&dir.path().join("test-model.bin"));

        tokio::fs::write(&part_path, &PAYLOAD[..5]).await.unwrap();
        download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(dir.path().join("test-model.bin"))
                .await
                .unwrap(),
            PAYLOAD
        );
    }

    #[tokio::test]
    async fn cancellation_preserves_the_partial_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_server, url) = serve_full().await;
        let cancel = CancellationToken::new();
        let final_path = dir.path().join("test-model.bin");
        let part_path = part_file(&final_path);
        tokio::fs::write(&part_path, &PAYLOAD[..5]).await.unwrap();

        // Pre-cancelled token: the loop must abort before touching anything.
        cancel.cancel();
        let error = download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(error, DownloadError::Cancelled));
        assert_eq!(tokio::fs::read(&part_path).await.unwrap(), &PAYLOAD[..5]);
        assert!(!final_path.exists());
    }

    #[tokio::test]
    async fn skips_download_when_final_file_already_verified() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_server, url) = serve_full().await;
        let cancel = CancellationToken::new();
        tokio::fs::write(dir.path().join("test-model.bin"), PAYLOAD)
            .await
            .unwrap();

        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_clone = std::sync::Arc::clone(&calls);

        download_file(
            &download_client(),
            dir.path(),
            &model_file(&url),
            &cancel,
            &|_, _, _| {
                calls_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .await
        .unwrap();

        // Progress reported once for the instant "already complete" skip.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
