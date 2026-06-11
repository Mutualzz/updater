use std::path::PathBuf;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Sha256, Digest};
use semver::Version;
use futures_util::StreamExt;
use log::{info, debug};

const LATEST_JSON_URL: &str = "https://proxy.mutualzz.com/releases/latest/latest.json";

#[derive(Debug, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(flatten)]
    pub platforms: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct PlatformAsset {
    url: String,
    sha256: String,
}

impl UpdateManifest {
    fn asset_for_current_platform(&self) -> Option<PlatformAsset> {
        let obj = self.platforms.as_object()?;

        #[cfg(target_os = "macos")]
        let (platform_key, arch_key) = ("osx", "universal");
        #[cfg(target_os = "windows")]
        let (platform_key, arch_key) = ("win", "x64");
        #[cfg(target_os = "linux")]
        let (platform_key, arch_key) = ("linux", "appimage");

        let asset = obj.get(platform_key)?.as_object()?.get(arch_key)?;
        serde_json::from_value(asset.clone()).ok()
    }
}

/// Returns the path to the persistent version file.
/// macOS:   ~/Library/Application Support/Mutualzz/version.txt
/// Windows: %APPDATA%\Mutualzz\version.txt
/// Linux:   ~/.local/share/Mutualzz/version.txt
pub fn version_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Mutualzz")
        .join("version.txt")
}

/// Reads the installed app version from the version file.
/// Returns "0.0.0" if the file doesn't exist (first install).
pub fn get_installed_version() -> String {
    std::fs::read_to_string(version_file_path())
        .unwrap_or_else(|_| "0.0.0".to_string())
        .trim()
        .to_string()
}

/// Writes the installed app version to the version file.
pub fn set_installed_version(version: &str) {
    let path = version_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, version.as_bytes()).ok();
    info!("Wrote installed version: {}", version);
}

pub async fn check_for_update() -> anyhow::Result<Option<UpdateManifest>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let manifest: UpdateManifest = client
        .get(LATEST_JSON_URL)
        .header("Cache-Control", "no-cache")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let remote = Version::parse(&manifest.version)?;

    // Use version file instead of CARGO_PKG_VERSION so the updater
    // binary version doesn't need to match the app version
    let installed = get_installed_version();
    let current = Version::parse(&installed)?;

    debug!("Remote: {}, installed: {}", remote, current);

    if remote > current {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

pub async fn download_update<F>(
    manifest: &UpdateManifest,
    mut on_progress: F,
) -> anyhow::Result<PathBuf>
where
    F: FnMut(f64, u64, u64, u64),
{
    let asset = manifest
        .asset_for_current_platform()
        .ok_or_else(|| anyhow::anyhow!("No asset for current platform"))?;

    info!("Downloading: {}", asset.url);

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let response = client.get(&asset.url).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();

    let tmp_dir = std::env::temp_dir().join("mutualzz-update");
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let file_name = asset.url.split('/').last().unwrap_or("update");
    let dest = tmp_dir.join(file_name);

    if dest.exists() {
        info!("Removing stale download: {}", dest.display());
        tokio::fs::remove_file(&dest).await.ok();
    }

    let mut file = tokio::fs::File::create(&dest).await?;
    let mut stream = response.bytes_stream();
    let start = std::time::Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;

        use tokio::io::AsyncWriteExt;
        file.write_all(&chunk).await?;

        let elapsed = start.elapsed().as_secs_f64();
        let bps = if elapsed > 0.0 { (downloaded as f64 / elapsed) as u64 } else { 0 };
        let percent = if total > 0 { (downloaded as f64 / total as f64) * 100.0 } else { 0.0 };
        on_progress(percent, bps, downloaded, total);
    }

    let hash = hex::encode(hasher.finalize());
    if hash != asset.sha256 {
        tokio::fs::remove_file(&dest).await.ok();
        return Err(anyhow::anyhow!(
            "Checksum mismatch: expected {}, got {}",
            asset.sha256,
            hash
        ));
    }

    info!("Download verified: {}", dest.display());
    Ok(dest)
}