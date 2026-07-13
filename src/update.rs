use futures_util::StreamExt;
use log::{debug, info, warn};
use reqwest::header::{HeaderValue, RANGE};
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const LATEST_JSON_URL: &str = "https://proxy.mutualzz.com/releases/latest/latest.json";

#[derive(Debug, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(flatten)]
    pub platforms: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AsarAsset {
    url: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct PlatformAsset {
    url: String,
    sha256: String,
    #[serde(rename = "electronVersion", default)]
    electron_version: Option<String>,
    #[serde(default)]
    asar: Option<AsarAsset>,
}

pub struct AsarUpdate {
    pub url: String,
    pub sha256: String,
}

impl UpdateManifest {
    fn asset_for_current_platform(&self) -> Option<PlatformAsset> {
        let obj = self.platforms.as_object()?;

        #[cfg(target_os = "macos")]
        {
            let asset = obj.get("osx")?.as_object()?.get("universal")?;
            return serde_json::from_value(asset.clone()).ok();
        }

        #[cfg(target_os = "windows")]
        {
            let asset = obj.get("win")?.as_object()?.get("x64")?;
            return serde_json::from_value(asset.clone()).ok();
        }

        #[cfg(target_os = "linux")]
        {
            let linux = obj.get("linux")?.as_object()?;
            let key = linux_package_key();
            let asset = linux
                .get(key)
                .or_else(|| linux.get("appimage"))
                .or_else(|| linux.get("debian"))?;
            return serde_json::from_value(asset.clone()).ok();
        }

        #[allow(unreachable_code)]
        None
    }

    pub fn electron_version_for_current_platform(&self) -> Option<String> {
        self.asset_for_current_platform()?.electron_version
    }

    pub fn asar_update(&self) -> Option<AsarUpdate> {
        let asset = self.asset_for_current_platform()?;
        let asar = asset.asar?;
        let remote_electron = asset.electron_version?;
        let installed_electron = get_installed_electron_version();

        if installed_electron.is_empty() || remote_electron != installed_electron {
            return None;
        }

        Some(AsarUpdate {
            url: asar.url,
            sha256: asar.sha256,
        })
    }
}

#[cfg(target_os = "linux")]
pub fn linux_package_key() -> &'static str {
    if std::env::var_os("APPIMAGE").is_some() {
        return "appimage";
    }

    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut id = String::new();
    let mut id_like = String::new();

    for line in os_release.lines() {
        if let Some(value) = line.strip_prefix("ID=") {
            id = value.trim_matches('"').to_ascii_lowercase();
        } else if let Some(value) = line.strip_prefix("ID_LIKE=") {
            id_like = value.trim_matches('"').to_ascii_lowercase();
        }
    }

    let haystack = format!("{id} {id_like}");

    const ARCH_IDS: &[&str] = &[
        "arch",
        "archarm",
        "manjaro",
        "endeavouros",
        "garuda",
        "cachyos",
        "artix",
        "archcraft",
        "arcolinux",
    ];
    if ARCH_IDS.iter().any(|candidate| id == *candidate) || haystack.contains("arch") {
        return "pacman";
    }

    const RPM_IDS: &[&str] = &[
        "fedora",
        "rhel",
        "centos",
        "rocky",
        "almalinux",
        "ol",
        "opensuse",
        "opensuse-tumbleweed",
        "opensuse-leap",
        "sles",
        "mageia",
        "nobara",
    ];
    if RPM_IDS.iter().any(|candidate| id == *candidate)
        || haystack
            .split_whitespace()
            .any(|token| matches!(token, "fedora" | "rhel" | "centos" | "suse"))
    {
        return "rpm";
    }

    const DEBIAN_IDS: &[&str] = &[
        "debian",
        "ubuntu",
        "linuxmint",
        "pop",
        "elementary",
        "zorin",
        "kali",
        "raspbian",
        "neon",
        "tails",
    ];
    if DEBIAN_IDS.iter().any(|candidate| id == *candidate)
        || haystack.contains("debian")
        || haystack.contains("ubuntu")
    {
        return "debian";
    }

    "appimage"
}

pub fn version_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Mutualzz")
        .join("version.txt")
}

pub fn get_installed_version() -> String {
    std::fs::read_to_string(version_file_path())
        .unwrap_or_else(|_| "0.0.0".to_string())
        .trim()
        .to_string()
}

pub fn set_installed_version(version: &str) {
    let path = version_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, version.as_bytes()).ok();
    info!("Wrote installed version: {}", version);
}

fn bundled_resource_path(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;

    #[cfg(target_os = "macos")]
    {
        let mac = crate::platform::install_dir()
            .join("Contents")
            .join("Resources")
            .join(name);
        if mac.is_file() {
            return Some(mac);
        }
    }

    let next_to_exe = exe_dir.join("resources").join(name);
    if next_to_exe.is_file() {
        return Some(next_to_exe);
    }

    let bare = exe_dir.join(name);
    if bare.is_file() {
        return Some(bare);
    }

    let in_install = crate::platform::install_dir()
        .join("resources")
        .join(name);
    if in_install.is_file() {
        return Some(in_install);
    }

    None
}

fn read_bundled_resource(name: &str) -> Option<String> {
    let path = bundled_resource_path(name)?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn seed_installed_versions_if_needed() {
    if !version_file_path().is_file() {
        if let Some(version) = read_bundled_resource("app-version.txt") {
            set_installed_version(&version);
        }
    }

    if get_installed_electron_version().is_empty() {
        if let Some(ev) = read_bundled_resource("electron-runtime-version.txt") {
            set_installed_electron_version(&ev);
        }
    }
}

pub fn electron_version_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Mutualzz")
        .join("electron-version.txt")
}

pub fn get_installed_electron_version() -> String {
    std::fs::read_to_string(electron_version_file_path())
        .unwrap_or_default()
        .trim()
        .to_string()
}

pub fn set_installed_electron_version(version: &str) {
    let path = electron_version_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, version.as_bytes()).ok();
    info!("Wrote installed Electron version: {}", version);
}

pub fn update_temp_dir() -> PathBuf {
    std::env::temp_dir().join("mutualzz-update")
}

pub fn cleanup_update_temp() {
    let dir = update_temp_dir();
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    info!("Cleaned update temp dir {}", dir.display());
}

pub async fn check_for_update() -> anyhow::Result<Option<UpdateManifest>> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(8))
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

    let installed = get_installed_version();
    let current = Version::parse(&installed)?;

    debug!("Remote: {}, installed: {}", remote, current);

    if remote > current {
        Ok(Some(manifest))
    } else {
        Ok(None)
    }
}

pub fn format_download_status(percent: f64, bps: u64, downloaded: u64, total: u64) -> String {
    let mb_dl = downloaded as f64 / 1_048_576.0;
    let speed = bps as f64 / 1_048_576.0;

    if total > 0 {
        let mb_total = total as f64 / 1_048_576.0;
        let eta = if bps > 0 && downloaded < total {
            let secs = (total - downloaded) / bps.max(1);
            format!("  ·  {}", format_eta(secs))
        } else {
            String::new()
        };
        format!("Downloading... {percent:.0}%  ({mb_dl:.1}/{mb_total:.1} MB){eta}")
    } else {
        format!("Downloading... {mb_dl:.1} MB ({speed:.1} MB/s)")
    }
}

fn format_eta(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s left")
    } else if secs < 3600 {
        format!("{}m {}s left", secs / 60, secs % 60)
    } else {
        format!("{}h {}m left", secs / 3600, (secs % 3600) / 60)
    }
}

async fn hash_existing_prefix(path: &Path, hasher: &mut Sha256) -> anyhow::Result<u64> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 1024 * 256];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok(total)
}

async fn download_and_verify<F>(
    url: &str,
    sha256_expected: &str,
    mut on_progress: F,
) -> anyhow::Result<PathBuf>
where
    F: FnMut(f64, u64, u64, u64),
{
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let tmp_dir = update_temp_dir();
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let file_name = url
        .split('/')
        .last()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("update");
    let dest = tmp_dir.join(file_name);
    let partial = tmp_dir.join(format!("{file_name}.partial"));

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut resume = false;

    if partial.is_file() {
        match hash_existing_prefix(&partial, &mut hasher).await {
            Ok(existing) if existing > 0 => {
                downloaded = existing;
                resume = true;
                info!(
                    "Resuming download from {} bytes ({})",
                    existing,
                    partial.display()
                );
            }
            _ => {
                warn!("Discarding unreadable partial {}", partial.display());
                let _ = tokio::fs::remove_file(&partial).await;
                hasher = Sha256::new();
                downloaded = 0;
            }
        }
    }

    if dest.is_file() {
        let _ = tokio::fs::remove_file(&dest).await;
    }

    let mut request = client.get(url);
    if resume && downloaded > 0 {
        request = request.header(
            RANGE,
            HeaderValue::from_str(&format!("bytes={downloaded}-"))?,
        );
    }

    let response = request.send().await?;
    let status = response.status();

    if resume && status == reqwest::StatusCode::PARTIAL_CONTENT {
    } else if status.is_success() {
        if resume {
            warn!("Server ignored resume request — restarting download");
            let _ = tokio::fs::remove_file(&partial).await;
            hasher = Sha256::new();
            downloaded = 0;
            resume = false;
        }
    } else {
        return Err(anyhow::anyhow!("HTTP {status}"));
    }

    let content_length = response.content_length().unwrap_or(0);
    let total = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        downloaded + content_length
    } else if content_length > 0 {
        content_length
    } else {
        0
    };

    let mut file = if resume && downloaded > 0 {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&partial)
            .await?
    } else {
        tokio::fs::File::create(&partial).await?
    };

    let mut stream = response.bytes_stream();
    let start = std::time::Instant::now();
    let started_bytes = downloaded;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        file.write_all(&chunk).await?;

        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        let bps = ((downloaded - started_bytes) as f64 / elapsed) as u64;
        let percent = if total > 0 {
            (downloaded as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        on_progress(percent, bps, downloaded, total);
    }

    file.flush().await?;
    drop(file);

    let hash = hex::encode(hasher.finalize());
    if hash != sha256_expected {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(anyhow::anyhow!(
            "Checksum mismatch: expected {}, got {}",
            sha256_expected,
            hash
        ));
    }

    tokio::fs::rename(&partial, &dest).await?;
    Ok(dest)
}

pub async fn download_update<F>(
    manifest: &UpdateManifest,
    on_progress: F,
) -> anyhow::Result<PathBuf>
where
    F: FnMut(f64, u64, u64, u64),
{
    let asset = manifest
        .asset_for_current_platform()
        .ok_or_else(|| anyhow::anyhow!("No asset for current platform"))?;

    info!("Downloading: {}", asset.url);
    let dest = download_and_verify(&asset.url, &asset.sha256, on_progress).await?;
    info!("Download verified: {}", dest.display());
    Ok(dest)
}

pub async fn download_asar_update<F>(asar: &AsarUpdate, on_progress: F) -> anyhow::Result<PathBuf>
where
    F: FnMut(f64, u64, u64, u64),
{
    info!("Downloading asar update: {}", asar.url);
    let dest = download_and_verify(&asar.url, &asar.sha256, on_progress).await?;
    info!("Asar download verified: {}", dest.display());
    Ok(dest)
}
