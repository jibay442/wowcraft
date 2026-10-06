//! Updates from the project's GitHub releases: the newest release's `WowCraft.zip`, downloaded and
//! unpacked by `curl` and `tar` (both in Windows 10 and 11; on Linux `unzip` or `bsdtar`), then
//! copied over the install. Only the programs are replaced, never the player's data: the database
//! (`mariadb/data`), the server's maps (`server/data`), and Prism with its sign-in and worlds.
//!
//! On Linux the launcher is an AppImage: the release's `WowCraft-x86_64.AppImage` (its own asset,
//! or inside the zip) replaces the one running.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::platform;

/// The Linux launcher's file in a release's zip.
pub const APPIMAGE: &str = "WowCraft-x86_64.AppImage";

/// The GitHub repository releases come from (`owner/name`), set when the package is built; empty
/// in a build without it, which then never looks for updates.
pub const REPO: &str = match option_env!("WOWCRAFT_UPDATE_REPO") {
    Some(r) => r,
    None => "",
};
/// This launcher's version (a release's tag is `v` and this).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A release newer than this launcher.
#[derive(Clone)]
pub struct Release {
    pub version: String,
    url: String,
    size: u64,
    /// The Linux launcher, when the release has it as an asset of its own (link and size).
    appimage: Option<(String, u64)>,
}

/// The newest release, if it's newer than this launcher (and GitHub answers: a private repository
/// or no network finds nothing).
pub fn check() -> Option<Release> {
    if REPO.is_empty() {
        return None;
    }
    let out = platform::command("curl")
        .args(["-sfL", "--max-time", "20", "-H", "User-Agent: WowCraft-launcher", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let json = String::from_utf8_lossy(&out.stdout);
    let version = string_after(&json, 0, "tag_name")?.trim_start_matches('v').to_string();
    if !newer(&version, VERSION) {
        return None;
    }
    let (url, size) = asset(&json, "WowCraft.zip")?;
    let appimage = if cfg!(windows) { None } else { asset(&json, APPIMAGE) };
    Some(Release { version, url, size, appimage })
}

/// A release asset's download link and size: its size follows its name, its link comes later.
fn asset(json: &str, name: &str) -> Option<(String, u64)> {
    let at = json.find(&format!("\"name\":\"{name}\"")).or_else(|| json.find(&format!("\"name\": \"{name}\"")))?;
    Some((string_after(json, at, "browser_download_url")?, number_after(json, at, "size")?))
}

/// Whether version `a` is newer than `b` (dotted numbers).
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// The string value of the first `"key":` at or after `from`.
fn string_after(json: &str, from: usize, key: &str) -> Option<String> {
    let rest = value_after(json, from, key)?;
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The number value of the first `"key":` at or after `from`.
fn number_after(json: &str, from: usize, key: &str) -> Option<u64> {
    let rest = value_after(json, from, key)?;
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn value_after<'a>(json: &'a str, from: usize, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let at = from + json.get(from..)?.find(&pat)? + pat.len();
    Some(json[at..].trim_start())
}

/// Downloads and unpacks `release` into `root\update`, telling `progress` how far (0..1); the
/// unpacked install is returned.
pub fn download(root: &Path, release: &Release, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    let dir = root.join("update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let zip = dir.join("WowCraft.zip");
    let total = release.size + release.appimage.as_ref().map_or(0, |a| a.1);
    fetch(&release.url, release.size, &zip, &|got| progress((got as f32 / total.max(1) as f32).min(1.0)))?;
    let unpacked = dir.join("x");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let ok = unzip(&zip, &unpacked);
    let new = unpacked.join("WowCraft");
    if !ok || !new.join("wow").is_dir() {
        return Err("The update couldn't be unpacked.".into());
    }
    // The Linux launcher, beside the rest as if it had come in the zip.
    if let Some((url, size)) = &release.appimage {
        let to = new.join(APPIMAGE);
        fetch(url, *size, &to, &|got| progress(((release.size + got) as f32 / total.max(1) as f32).min(1.0)))?;
    }
    Ok(new)
}

/// Downloads `url` (`size` bytes) to `to`, telling `progress` the bytes so far.
fn fetch(url: &str, size: u64, to: &Path, progress: &dyn Fn(u64)) -> Result<(), String> {
    let mut curl = platform::command("curl")
        .args(["-sfL", "-H", "User-Agent: WowCraft-launcher", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("couldn't start the download: {e}"))?;
    let status = loop {
        if let Some(status) = curl.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        progress(std::fs::metadata(to).map(|m| m.len()).unwrap_or(0));
        std::thread::sleep(Duration::from_millis(250));
    };
    let got = std::fs::metadata(to).map(|m| m.len()).unwrap_or(0);
    if !status.success() || got != size {
        return Err("The download failed. Check your connection and try again.".into());
    }
    Ok(())
}

/// Copies the new programs from `new` over the install at `root` (with nothing running); then
/// [`restart`].
pub fn apply(root: &Path, new: &Path) -> Result<(), String> {
    let copy = |rel: &str| copy_tree(&new.join(rel), &root.join(rel)).map_err(|e| format!("couldn't update {rel}: {e}"));
    // The bundled mods are ours alone: older jars go, so a renamed one isn't loaded twice.
    let mods = root.join("minecraft").join("WowCraft").join("minecraft").join("mods");
    if let Ok(old) = std::fs::read_dir(&mods) {
        for e in old.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    for rel in ["wow", "minecraft", "licenses", "mariadb/data-clean"] {
        if new.join(rel).exists() {
            copy(rel)?;
        }
    }
    // The server's programs and configs, not its data folder (the maps made from the player's WoW).
    for e in std::fs::read_dir(new.join("server")).map_err(|e| e.to_string())?.flatten() {
        if e.path().is_file() {
            std::fs::copy(e.path(), root.join("server").join(e.file_name())).map_err(|err| format!("couldn't update the server: {err}"))?;
        }
    }
    let _ = std::fs::copy(new.join("README.txt"), root.join("README.txt"));
    // Programs keep running from a file that's renamed, not one overwritten: the old launcher
    // steps aside.
    let (exe, old, fresh) = launcher_files(root, new);
    if !fresh.is_file() {
        // A release without a Linux launcher: the game is updated, this launcher stays.
        return Ok(());
    }
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("couldn't replace the launcher: {e}"))?;
    if let Err(e) = std::fs::copy(&fresh, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("couldn't replace the launcher: {e}"));
    }
    platform::make_executable(&exe);
    Ok(())
}

/// The launcher's file, where it steps aside to, and its new copy in the unpacked release `new`:
/// `WowCraft.exe` on Windows, the running AppImage on Linux.
fn launcher_files(root: &Path, new: &Path) -> (PathBuf, PathBuf, PathBuf) {
    if cfg!(windows) {
        return (root.join("WowCraft.exe"), root.join("WowCraft.old.exe"), new.join("WowCraft.exe"));
    }
    let exe = launcher(root);
    let old = exe.with_extension("AppImage.old");
    (exe, old, new.join(APPIMAGE))
}

/// The launcher's own file: `$APPIMAGE` (set by the AppImage runtime), else this program.
fn launcher(root: &Path) -> PathBuf {
    if cfg!(windows) {
        return root.join("WowCraft.exe");
    }
    std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok()).unwrap_or_else(|| root.join(APPIMAGE))
}

/// Unpacks `zip` into `to`: `tar` on Windows (it reads zips there); `unzip`, else `bsdtar`, on Linux.
fn unzip(zip: &Path, to: &Path) -> bool {
    let run = |tool: &str, args: &[&std::ffi::OsStr]| platform::command(tool).args(args).stdin(Stdio::null()).status().is_ok_and(|s| s.success());
    let (zip, to) = (zip.as_os_str(), to.as_os_str());
    let tar = || run("tar", &["-xf".as_ref(), zip, "-C".as_ref(), to]);
    if cfg!(windows) {
        return tar();
    }
    run("unzip", &["-q".as_ref(), "-o".as_ref(), zip, "-d".as_ref(), to]) || run("bsdtar", &["-xf".as_ref(), zip, "-C".as_ref(), to])
}

/// Starts the (new) launcher; this one should exit right after.
pub fn restart(root: &Path) -> Result<(), String> {
    Command::new(launcher(root)).current_dir(root).spawn().map_err(|e| format!("couldn't start the new launcher: {e}"))?;
    Ok(())
}

/// What an update leaves behind: the old launcher and the download.
pub fn clean_up(root: &Path) {
    let (_, old, _) = launcher_files(root, root);
    let _ = std::fs::remove_file(old);
    let _ = std::fs::remove_dir_all(root.join("update"));
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        copy_tree(&e.path(), &to.join(e.file_name()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn the_release_json_gives_version_size_and_link() {
        let json = r#"{"tag_name":"v0.2.0","assets":[{"name":"Other.zip","size":5,"browser_download_url":"https://x/Other.zip"},{"name":"WowCraft.zip","uploader":{"login":"a"},"size":1234,"browser_download_url":"https://x/WowCraft.zip"}]}"#;
        assert_eq!(string_after(json, 0, "tag_name").as_deref(), Some("v0.2.0"));
        assert_eq!(asset(json, "WowCraft.zip"), Some(("https://x/WowCraft.zip".into(), 1234)));
        assert_eq!(asset(json, APPIMAGE), None);
    }

    /// The real package (WOWCRAFT_TEST_ZIP) downloads, unpacks and lands over an install whose
    /// data stays: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_update_keeps_the_data() {
        let zip = PathBuf::from(std::env::var("WOWCRAFT_TEST_ZIP").expect("WOWCRAFT_TEST_ZIP"));
        let root = std::env::temp_dir().join(format!("wowcraft-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["mariadb/data", "server/data", "minecraft/WowCraft/minecraft/mods", "prism", "wow"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("mariadb/data/characters.marker"), "mine").unwrap();
        std::fs::write(root.join("server/data/maps.marker"), "mine").unwrap();
        std::fs::write(root.join("prism/accounts.json"), "mine").unwrap();
        std::fs::write(root.join("minecraft/WowCraft/minecraft/mods/wowcraft-0.0.1.jar"), "old").unwrap();
        std::fs::write(root.join("WowCraft.exe"), "old launcher").unwrap();
        std::fs::write(root.join("wow/benilla.exe"), "old").unwrap();
        let size = std::fs::metadata(&zip).unwrap().len();
        let url = format!("file:///{}", zip.display().to_string().replace('\\', "/"));
        let release = Release { version: "9.9.9".into(), url, size, appimage: None };
        let new = download(&root, &release, &|_| {}).unwrap();
        apply(&root, &new).unwrap();
        for (f, want) in [("mariadb/data/characters.marker", "mine"), ("server/data/maps.marker", "mine"), ("prism/accounts.json", "mine")] {
            assert_eq!(std::fs::read_to_string(root.join(f)).unwrap(), want, "{f} kept");
        }
        assert!(!root.join("minecraft/WowCraft/minecraft/mods/wowcraft-0.0.1.jar").exists(), "old mod gone");
        assert!(std::fs::read_dir(root.join("minecraft/WowCraft/minecraft/mods")).unwrap().count() >= 3, "new mods in");
        assert!(std::fs::metadata(root.join("WowCraft.exe")).unwrap().len() > 1000, "new launcher in");
        assert!(std::fs::metadata(root.join("wow/benilla.exe")).unwrap().len() > 1000, "new WoW in");
        assert!(root.join("mariadb/data-clean/realmd").is_dir(), "empty database beside");
        assert!(root.join("WowCraft.old.exe").exists());
        clean_up(&root);
        assert!(!root.join("update").exists() && !root.join("WowCraft.old.exe").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
