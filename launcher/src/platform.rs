//! What differs between Windows and Linux: starting programs without a console window, seeing
//! which programs run, ending them, and running the install's programs. On Linux a program of
//! the install runs as a Linux build when there is one, else (for the database) the system's own,
//! else its Windows `.exe` through Wine.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Line ends for the config files the server reads.
#[cfg(windows)]
pub const NEWLINE: &str = "\r\n";
#[cfg(not(windows))]
pub const NEWLINE: &str = "\n";

/// A command that opens no console window (Windows) and keeps Wine quiet (Linux).
pub fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs `cmd` at the lowest priority (on Linux, the caller starts it through `nice`), in a process
/// group of its own.
pub fn background(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
        cmd.creation_flags(CREATE_NO_WINDOW | IDLE_PRIORITY_CLASS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// One of the install's programs, and how it runs.
#[derive(Clone, Debug)]
pub struct Program {
    pub path: PathBuf,
    /// A Windows program run through Wine.
    pub wine: bool,
    /// Part of the install (a system-wide database instead needs no `--basedir`).
    pub bundled: bool,
}

impl Program {
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// The command that starts it (through Wine if it needs it).
    pub fn command(&self) -> Command {
        if !self.wine {
            return command(&self.path);
        }
        // Wine makes the prefix's own folder, not the folders above it.
        let prefix = wine_prefix();
        if let Some(parent) = prefix.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut cmd = command(wine().unwrap_or_else(|| PathBuf::from("wine")));
        cmd.arg(&self.path)
            .env("WINEPREFIX", prefix)
            .env("WINEDEBUG", "-all")
            // No Mono or Gecko installer asking on the first start.
            .env("WINEDLLOVERRIDES", "mscoree,mshtml=");
        cmd
    }

    /// A path as this program understands it: a Windows one (on Wine's Z: drive) for Wine.
    pub fn path_arg(&self, path: &Path) -> String {
        if self.wine { windows_path(path) } else { path.display().to_string() }
    }

    /// Its process name, as [`running`] lists it.
    pub fn image(&self) -> String {
        image_name(&self.path.display().to_string())
    }

    /// A program other programs (Minecraft's mod) can start themselves: itself, or for a Wine one
    /// a small script at `wrapper` that starts it through Wine.
    pub fn native(&self, wrapper: &Path) -> PathBuf {
        if !self.wine {
            return self.path.clone();
        }
        let wine = wine().unwrap_or_else(|| PathBuf::from("wine"));
        if let Some(parent) = wine_prefix().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let script = format!(
            "#!/bin/sh\nexport WINEPREFIX={} WINEDEBUG=-all WINEDLLOVERRIDES='mscoree,mshtml='\nexec {} {} \"$@\"\n",
            shell_quote(&wine_prefix().display().to_string()),
            shell_quote(&wine.display().to_string()),
            shell_quote(&self.path.display().to_string()),
        );
        if let Some(dir) = wrapper.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(wrapper, script);
        make_executable(wrapper);
        wrapper.to_path_buf()
    }
}

/// The install's program `name` in `dir`. Windows: `name.exe`. Linux: `name` itself, else the
/// first of `system` found on the PATH, else `name.exe` through Wine.
pub fn program(dir: &Path, name: &str, system: &[&str]) -> Program {
    let exe = dir.join(format!("{name}.exe"));
    if cfg!(windows) {
        return Program { path: exe, wine: false, bundled: true };
    }
    let native = dir.join(name);
    if native.is_file() {
        return Program { path: native, wine: false, bundled: true };
    }
    if let Some(path) = system.iter().find_map(|s| on_path(s)) {
        return Program { path, wine: false, bundled: false };
    }
    Program { path: exe, wine: true, bundled: true }
}

/// `name` on the PATH (and in the sbin folders, where distributions put a database server).
pub fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/local/sbin", "/usr/sbin", "/sbin"].map(PathBuf::from))
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Wine: `$WINE`, else `wine` on the PATH.
pub fn wine() -> Option<PathBuf> {
    std::env::var_os("WINE").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| on_path("wine"))
}

/// The Wine prefix WowCraft's Windows programs run in: `$WINEPREFIX`, else one of its own (so the
/// player's ~/.wine is left alone).
pub fn wine_prefix() -> PathBuf {
    std::env::var_os("WINEPREFIX").map(PathBuf::from).unwrap_or_else(|| data_home().join("WowCraft").join("wine"))
}

/// `$XDG_DATA_HOME`, else ~/.local/share.
pub fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local").join("share"))
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// A Linux path as Windows programs under Wine see it (Wine's Z: drive is /).
pub fn windows_path(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    format!("Z:{}", abs.display().to_string().replace('/', "\\"))
}

/// The process name of a program's path, lowercased and without `.exe`: `C:\x\WoW.exe`,
/// `Z:\home\x\benilla.exe` and `/opt/x/benilla` are `wow`, `benilla` and `benilla`.
pub fn image_name(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(unix)]
pub fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}
#[cfg(not(unix))]
pub fn make_executable(_path: &Path) {}

/// Every process: its id and its name ([`image_name`]).
pub fn processes() -> Vec<(u32, String)> {
    #[cfg(windows)]
    {
        let out = command("tasklist").args(["/FO", "CSV", "/NH"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        out.lines()
            .filter_map(|l| {
                let mut cols = l.split(',').map(|c| c.trim_matches('"'));
                let image = cols.next()?;
                let pid = cols.next()?.parse().ok()?;
                Some((pid, image_name(image)))
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        proc_pids().into_iter().filter_map(|pid| Some((pid, linux_image(pid)?))).collect()
    }
}

/// Which processes are up, by [`image_name`].
pub fn running() -> Vec<String> {
    processes().into_iter().map(|(_, name)| name).collect()
}

/// The ids of the processes named `image`.
#[cfg(not(windows))]
fn pids(image: &str) -> Vec<u32> {
    processes().into_iter().filter(|(_, n)| n == image).map(|(pid, _)| pid).collect()
}

/// Ends the processes named `image`: asks them to quit, or (`force`) makes them.
pub fn kill(image: &str, force: bool) {
    #[cfg(windows)]
    {
        let exe = format!("{image}.exe");
        let mut args = vec!["/IM", exe.as_str()];
        if force {
            args.push("/F");
        }
        let _ = command("taskkill").args(args).status();
    }
    #[cfg(not(windows))]
    for pid in pids(image) {
        signal(&pid.to_string(), force);
    }
}

/// Ends process `pid` and everything it started.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = command("taskkill").args(["/T", "/F", "/PID", &pid.to_string()]).status();
    }
    #[cfg(not(windows))]
    {
        // Its children, theirs, and so on (from each process's parent in /proc/<pid>/stat).
        let parents: Vec<(u32, u32)> = proc_pids()
            .into_iter()
            .filter_map(|p| {
                let stat = std::fs::read_to_string(format!("/proc/{p}/stat")).ok()?;
                // After the name in brackets: state, then the parent's id.
                let ppid = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
                Some((p, ppid))
            })
            .collect();
        let mut tree = vec![pid];
        let mut i = 0;
        while i < tree.len() {
            let parent = tree[i];
            tree.extend(parents.iter().filter(|(_, pp)| *pp == parent).map(|(p, _)| *p));
            i += 1;
        }
        for p in tree {
            signal(&p.to_string(), true);
        }
    }
}

#[cfg(not(windows))]
fn signal(target: &str, force: bool) {
    let _ = command("kill").args([if force { "-KILL" } else { "-TERM" }, "--", target]).stderr(std::process::Stdio::null()).status();
}

#[cfg(not(windows))]
fn proc_pids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

/// A Linux process's name: its program's, or for one under Wine the Windows program's.
#[cfg(not(windows))]
fn linux_image(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv0 = cmdline.split(|&b| b == 0).next().map(|a| String::from_utf8_lossy(a).to_string()).unwrap_or_default();
    if !argv0.is_empty() {
        return Some(image_name(&argv0));
    }
    // Kernel threads and the like.
    std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|c| image_name(c.trim()))
}

/// Whether some program has `file` open. Windows: someone holds it so nobody else can open it alone
/// (as Minecraft does its log). Linux: a process's open files include it.
pub fn held_open(file: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new().read(true).share_mode(0).open(file) {
            Ok(_) => false,
            // ERROR_SHARING_VIOLATION: someone has it open.
            Err(e) => e.raw_os_error() == Some(32),
        }
    }
    #[cfg(not(windows))]
    {
        let Ok(file) = std::fs::canonicalize(file) else { return false };
        proc_pids().into_iter().any(|pid| {
            std::fs::read_dir(format!("/proc/{pid}/fd"))
                .is_ok_and(|fds| fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|target| target == file)))
        })
    }
}

/// The program listening on a local TCP port (its path), if it can be told.
pub fn port_owner(port: u16) -> Option<String> {
    #[cfg(windows)]
    {
        let out = command("netstat").args(["-ano", "-p", "TCP"]).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let pid = text.lines().find_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            (cols.len() >= 5 && cols[3] == "LISTENING" && cols[1].ends_with(&format!(":{port}"))).then(|| cols[4].to_string())
        })?;
        let path = command("powershell").args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid}).Path")]).output().ok()?;
        let path = String::from_utf8_lossy(&path.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }
    #[cfg(not(windows))]
    {
        // The listening sockets' inodes, then the process holding one of them.
        let inodes: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .flat_map(|text| {
                text.lines()
                    .skip(1)
                    .filter_map(|line| {
                        let cols: Vec<&str> = line.split_whitespace().collect();
                        let local_port = u16::from_str_radix(cols.get(1)?.rsplit(':').next()?, 16).ok()?;
                        let inode = cols.get(9)?;
                        (local_port == port && *cols.get(3)? == "0A").then(|| format!("socket:[{inode}]"))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let pid = proc_pids().into_iter().find(|pid| {
            std::fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|fds| {
                fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| inodes.iter().any(|i| t.as_os_str() == i.as_str())))
            })
        })?;
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        if !exe.display().to_string().contains("wine") {
            return Some(exe.display().to_string());
        }
        // A Windows program under Wine: its Windows path, back on the Linux side.
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let argv0 = String::from_utf8_lossy(cmdline.split(|&b| b == 0).next()?).to_string();
        let unix = argv0.strip_prefix("Z:").or_else(|| argv0.strip_prefix("z:")).map(|p| p.replace('\\', "/"));
        Some(unix.unwrap_or(argv0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_names_drop_folders_and_exe() {
        assert_eq!(image_name(r"C:\x\WoW.exe"), "wow");
        assert_eq!(image_name(r"Z:\home\me\WowCraft\wow\benilla.exe"), "benilla");
        assert_eq!(image_name("/usr/sbin/mariadbd"), "mariadbd");
        assert_eq!(image_name("javaw.exe"), "javaw");
    }

    #[cfg(not(windows))]
    #[test]
    fn wine_sees_linux_paths_on_z() {
        assert_eq!(windows_path(Path::new("/home/me/WoW/Data")), r"Z:\home\me\WoW\Data");
    }

    /// Linux: a file counts as open while this process holds it.
    #[cfg(not(windows))]
    #[test]
    fn an_open_file_is_seen() {
        let path = std::env::temp_dir().join(format!("wowcraft-open-{}", std::process::id()));
        let held = std::fs::File::create(&path).unwrap();
        assert!(held_open(&path));
        drop(held);
        assert!(!held_open(&path));
        let _ = std::fs::remove_file(path);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_listening_port_has_an_owner() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let owner = port_owner(port).expect("owner found");
        assert_eq!(std::path::PathBuf::from(owner), std::env::current_exe().unwrap());
    }

    /// A program of the install: its Linux build if there is one, else its .exe through Wine.
    #[cfg(not(windows))]
    #[test]
    fn linux_builds_beat_wine() {
        let dir = std::env::temp_dir().join(format!("wowcraft-prog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mangosd.exe"), "").unwrap();
        let p = program(&dir, "mangosd", &[]);
        assert!(p.wine && p.path == dir.join("mangosd.exe"));
        std::fs::write(dir.join("mangosd"), "").unwrap();
        let p = program(&dir, "mangosd", &[]);
        assert!(!p.wine && p.path == dir.join("mangosd"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
