//! Updates from the project's GitHub releases: the newest release's `WowCraft.zip`, downloaded and
//! unpacked by Windows' own `curl.exe` and `tar.exe` (both in Windows 10 and 11), then copied over
//! the install. Only the programs are replaced, never the player's data: the database
//! (`mariadb\data`), the server's maps (`server\data`), and Prism with its sign-in and worlds.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::CREATE_NO_WINDOW;

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
}

/// The newest release, if it's newer than this launcher (and GitHub answers: a private repository
/// or no network finds nothing).
pub fn check() -> Option<Release> {
    if REPO.is_empty() {
        return None;
    }
    let out = Command::new("curl.exe")
        .args(["-sfL", "--max-time", "20", "-H", "User-Agent: WowCraft-launcher", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
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
    // The asset named WowCraft.zip: its size follows its name, its download link comes later.
    let at = json.find("\"name\":\"WowCraft.zip\"").or_else(|| json.find("\"name\": \"WowCraft.zip\""))?;
    let size = number_after(&json, at, "size")?;
    let url = string_after(&json, at, "browser_download_url")?;
    Some(Release { version, url, size })
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
    let mut curl = Command::new("curl.exe")
        .args(["-sfL", "-H", "User-Agent: WowCraft-launcher", "-o"])
        .arg(&zip)
        .arg(&release.url)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("couldn't start the download: {e}"))?;
    let status = loop {
        if let Some(status) = curl.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let got = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
        progress((got as f32 / release.size.max(1) as f32).min(1.0));
        std::thread::sleep(Duration::from_millis(250));
    };
    let got = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
    if !status.success() || got != release.size {
        return Err("The download failed. Check your connection and try again.".into());
    }
    let unpacked = dir.join("x");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let ok = Command::new("tar.exe")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unpacked)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success());
    let new = unpacked.join("WowCraft");
    if !ok || !new.join("WowCraft.exe").exists() {
        return Err("The update couldn't be unpacked.".into());
    }
    Ok(new)
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
    for rel in ["wow", "minecraft", "licenses", "mariadb\\data-clean"] {
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
    // A running program can be renamed, not overwritten: the old launcher steps aside.
    let exe = root.join("WowCraft.exe");
    let old = root.join("WowCraft.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("couldn't replace the launcher: {e}"))?;
    if let Err(e) = std::fs::copy(new.join("WowCraft.exe"), &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("couldn't replace the launcher: {e}"));
    }
    Ok(())
}

/// Starts the (new) launcher; this one should exit right after.
pub fn restart(root: &Path) -> Result<(), String> {
    Command::new(root.join("WowCraft.exe")).current_dir(root).spawn().map_err(|e| format!("couldn't start the new launcher: {e}"))?;
    Ok(())
}

/// What an update leaves behind: the old launcher and the download.
pub fn clean_up(root: &Path) {
    let _ = std::fs::remove_file(root.join("WowCraft.old.exe"));
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
        let at = json.find("\"name\":\"WowCraft.zip\"").unwrap();
        assert_eq!(number_after(json, at, "size"), Some(1234));
        assert_eq!(string_after(json, at, "browser_download_url").as_deref(), Some("https://x/WowCraft.zip"));
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
        let release = Release { version: "9.9.9".into(), url, size };
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
