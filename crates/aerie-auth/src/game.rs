//! The game package: the files a server needs (the assets, the server jar),
//! fetched with the account's own login, as the official server's update
//! module does (spec 26). The account service lists the release channels
//! ("patchlines") the account may use and hands out short-lived signed links
//! to a channel's version manifest and to its package; the manifest gives
//! the package's SHA-256, checked after the download.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn};

use crate::oauth::Tokens;
use crate::{ACCOUNT_DATA, Error, Result, USER_AGENT, check};

/// The channel the official server follows when none is set.
pub const DEFAULT_PATCHLINE: &str = "release";

/// A release channel the account may use.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Patchline {
    pub name: String,
    /// When the account stops having it (milliseconds since the epoch), or 0.
    #[serde(default)]
    pub expires_at: i64,
}

#[derive(Deserialize)]
struct Patchlines {
    #[serde(default)]
    patchlines: Vec<Patchline>,
}

/// A channel's current version and where its package is.
#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// The package's path in the account service (not a link: it is signed
    /// separately, see `signed_url`).
    pub download_url: String,
    pub sha256: Option<String>,
}

#[derive(Deserialize)]
struct Signed {
    url: String,
}

/// The channels this account may download.
pub async fn patchlines(http: &reqwest::Client, oauth: &Tokens) -> Result<Vec<Patchline>> {
    let r = http
        .get(format!("{ACCOUNT_DATA}/my-account/get-patchlines"))
        .bearer_auth(&oauth.access_token)
        .header("Accept", "application/json")
        .send()
        .await?;
    Ok(check("patchlines", r)
        .await?
        .json::<Patchlines>()
        .await?
        .patchlines)
}

/// A short-lived link to a file of the game's storage.
pub async fn signed_url(http: &reqwest::Client, oauth: &Tokens, path: &str) -> Result<String> {
    let r = http
        .get(format!("{ACCOUNT_DATA}/game-assets/{path}"))
        .bearer_auth(&oauth.access_token)
        .header("Accept", "application/json")
        .send()
        .await?;
    Ok(check("signed link", r).await?.json::<Signed>().await?.url)
}

/// The current version of a channel.
pub async fn manifest(http: &reqwest::Client, oauth: &Tokens, patchline: &str) -> Result<Manifest> {
    let url = signed_url(http, oauth, &format!("version/{patchline}.json")).await?;
    let r = http
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await?;
    Ok(check("version manifest", r)
        .await?
        .json::<Manifest>()
        .await?)
}

/// A client for large downloads: no limit on the whole transfer (the
/// package is gigabytes), only on connecting and on a stalled read.
pub fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("http client")
}

/// Progress of a download: bytes on disk, and the total once known.
pub type Progress<'a> = &'a mut (dyn FnMut(u64, Option<u64>) + Send);

/// How many times a dropped download is resumed before giving up.
const ATTEMPTS: u32 = 8;

/// Downloads a manifest's package to `dest`, resuming from what an earlier
/// attempt left in `dest.part`, then checks its SHA-256. Each attempt asks
/// for a new signed link, since links expire.
pub async fn download(
    http: &reqwest::Client,
    oauth: &Tokens,
    manifest: &Manifest,
    dest: &Path,
    progress: Progress<'_>,
) -> Result<()> {
    let part = dest.with_extension("part");
    let mut attempt = 0;
    loop {
        attempt += 1;
        match fetch_into(http, oauth, manifest, &part, progress).await {
            Ok(()) => break,
            Err(e) if attempt < ATTEMPTS && retryable(&e) => {
                warn!(
                    "download interrupted ({e}); resuming, attempt {}",
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
    if let Some(want) = &manifest.sha256 {
        info!("checking the package's SHA-256");
        let got = sha256_file(&part).await?;
        if !got.eq_ignore_ascii_case(want) {
            // A bad file is not resumed from: the next try starts over.
            tokio::fs::remove_file(&part).await?;
            return Err(Error::BadPackage(format!("SHA-256 {got}, expected {want}")));
        }
    }
    tokio::fs::rename(&part, dest).await?;
    Ok(())
}

fn retryable(e: &Error) -> bool {
    match e {
        Error::Http(_) | Error::Io(_) => true,
        Error::Status { status, .. } => *status >= 500 || *status == 403,
        _ => false,
    }
}

async fn fetch_into(
    http: &reqwest::Client,
    oauth: &Tokens,
    manifest: &Manifest,
    part: &Path,
    progress: Progress<'_>,
) -> Result<()> {
    let have = match tokio::fs::metadata(part).await {
        Ok(m) => m.len(),
        Err(_) => 0,
    };
    let url = signed_url(http, oauth, &manifest.download_url).await?;
    let mut req = http.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut r = check("package", req.send().await?).await?;
    // 206: the rest of the file; 200: the whole file (the store ignored the
    // range), so start over.
    let resumed = have > 0 && r.status().as_u16() == 206;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)
        .await?;
    let mut done = if resumed { have } else { 0 };
    let total = r.content_length().map(|n| n + done);
    progress(done, total);
    while let Some(chunk) = r.chunk().await? {
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await?;
    if let Some(t) = total
        && done != t
    {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("got {done} of {t} bytes"),
        )));
    }
    Ok(())
}

/// The SHA-256 of a file, in lowercase hex.
pub async fn sha256_file(path: &Path) -> Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answers() {
        let p: Patchlines = serde_json::from_str(
            r#"{"patchlines":[{"name":"release","expiresAt":0},{"name":"pre"}]}"#,
        )
        .unwrap();
        assert_eq!(p.patchlines.len(), 2);
        assert_eq!(p.patchlines[1].expires_at, 0);
        let m: Manifest = serde_json::from_str(
            r#"{"version":"1.2.3","download_url":"builds/x.zip","sha256":"AB"}"#,
        )
        .unwrap();
        assert_eq!(m.download_url, "builds/x.zip");
        let s: Signed = serde_json::from_str(r#"{"url":"https://example.invalid/a"}"#).unwrap();
        assert!(s.url.starts_with("https://"));
    }

    #[tokio::test]
    async fn hashes_a_file() {
        let dir = std::env::temp_dir().join(format!("aerie-hash-{}", std::process::id()));
        tokio::fs::write(&dir, b"abc").await.unwrap();
        assert_eq!(
            sha256_file(&dir).await.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        tokio::fs::remove_file(&dir).await.unwrap();
    }
}
