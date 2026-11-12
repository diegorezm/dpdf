//! Update check against GitHub releases. The only network access Leme
//! makes, and only to github.com: one request on startup (when
//! `check_updates` is on) or when asked from the palette. Nothing is
//! downloaded until the user says yes.
//!
//! Requests go through Windows' own `curl.exe` (System32, Windows 10+),
//! so Leme carries no HTTP or TLS stack of its own.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "https://github.com/diegorezm/leme";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// Without the leading `v`, e.g. `1.2.0`.
    pub version: String,
}

impl Release {
    pub fn page_url(&self) -> String {
        format!("{REPO}/releases/tag/v{}", self.version)
    }

    fn installer_name(&self) -> String {
        format!("leme-{}-setup.exe", self.version)
    }

    fn asset_url(&self, name: &str) -> String {
        format!("{REPO}/releases/download/v{}/{name}", self.version)
    }
}

/// The latest release, if it is newer than this build. Blocks; call it
/// off the UI thread.
pub fn check() -> io::Result<Option<Release>> {
    // /releases/latest redirects to /releases/tag/v<version>.
    let output = curl()
        .args(["-o", "NUL", "-w", "%{redirect_url}"])
        .arg(format!("{REPO}/releases/latest"))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("couldn't reach GitHub"));
    }
    let location = String::from_utf8_lossy(&output.stdout);
    let latest = version_from_url(&location).ok_or_else(|| io::Error::other("no release found"))?;
    Ok(is_newer(&latest, env!("CARGO_PKG_VERSION")).then_some(Release { version: latest }))
}

/// Downloads the release's installer to the temp folder and checks it
/// against the release's SHA256SUMS.txt. Blocks.
pub fn download(release: &Release) -> io::Result<PathBuf> {
    let name = release.installer_name();
    let target = std::env::temp_dir().join(&name);
    let sums = curl()
        .arg("-L")
        .arg(release.asset_url("SHA256SUMS.txt"))
        .output()?;
    let expected = sums
        .status
        .success()
        .then(|| checksum_for(&String::from_utf8_lossy(&sums.stdout), &name))
        .flatten()
        .ok_or_else(|| io::Error::other("the release has no checksum for its installer"))?;

    let status = curl()
        .args(["-L", "-o"])
        .arg(&target)
        .arg(release.asset_url(&name))
        .status()?;
    if !status.success() {
        return Err(io::Error::other("download failed"));
    }
    if sha256(&target)? != expected {
        let _ = fs::remove_file(&target);
        return Err(io::Error::other("the download doesn't match its checksum"));
    }
    Ok(target)
}

/// Starts the installer quietly; it closes Leme and opens it again when
/// done. The caller should quit right after.
pub fn install(installer: &Path) -> io::Result<()> {
    Command::new(installer)
        .args(["/SILENT", "/SP-", "/NOCANCEL", "/CLOSEAPPLICATIONS"])
        .spawn()
        .map(|_| ())
}

/// Whether this copy was installed by the installer, rather than unzipped.
/// Only installed copies update in place.
pub fn installed() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("unins000.exe")))
        .is_some_and(|uninstaller| uninstaller.exists())
}

/// Opens a release page in the default browser.
pub fn open_page(release: &Release) {
    let _ = Command::new("explorer").arg(release.page_url()).spawn();
}

/// The version the user chose to skip, if any.
pub fn skipped() -> Option<String> {
    let text = fs::read_to_string(skip_file()?).ok()?;
    Some(text.trim().to_owned()).filter(|v| !v.is_empty())
}

pub fn skip(release: &Release) -> io::Result<()> {
    let path = skip_file().ok_or_else(|| io::Error::other("LOCALAPPDATA is not set"))?;
    crate::config::write_atomic(&path, &release.version)
}

fn skip_file() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("leme")
            .join("skipped-update.txt"),
    )
}

fn curl() -> Command {
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut command = Command::new(Path::new(&system).join("System32").join("curl.exe"));
    command.args([
        "-sSf",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--max-time",
        "120",
    ]);
    no_window(&mut command);
    command
}

fn sha256(path: &Path) -> io::Result<String> {
    let mut command = Command::new("certutil");
    command.arg("-hashfile").arg(path).arg("SHA256");
    no_window(&mut command);
    let output = command.output()?;
    // Line 2 is the hash; older Windows separates its bytes with spaces.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .nth(1)
        .map(|line| line.replace(' ', "").to_lowercase())
        .filter(|hash| hash.len() == 64)
        .ok_or_else(|| io::Error::other("couldn't hash the download"))
}

/// Keeps console tools from flashing a window.
fn no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

/// `1.2.0` from `https://github.com/o/r/releases/tag/v1.2.0`.
fn version_from_url(url: &str) -> Option<String> {
    let tag = url.trim().rsplit_once("/releases/tag/")?.1;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse(version).map(|_| version.to_owned())
}

fn parse(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The hash for `name` in a `sha256sum`-style listing.
fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim() == name && hash.len() == 64).then(|| hash.to_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            version_from_url("https://github.com/d/leme/releases/tag/v1.10.0\n").as_deref(),
            Some("1.10.0")
        );
        assert_eq!(version_from_url("https://github.com/d/leme/releases"), None);
        assert_eq!(version_from_url("https://x/releases/tag/vnext"), None);
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("garbage", "1.0.0"));
    }

    #[test]
    fn checksums() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{hash}  leme-1.1.0-setup.exe\n{}  other.zip\n",
            "b".repeat(64)
        );
        assert_eq!(checksum_for(&sums, "leme-1.1.0-setup.exe"), Some(hash));
        assert_eq!(checksum_for(&sums, "missing.exe"), None);
    }
}
