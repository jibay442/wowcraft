//! The packaged install's own WoW server: its database (on its own port, so a MySQL of the
//! player's doesn't clash), its config files pointed at this folder, and its map data, which is
//! extracted from the player's own WoW client the first time (WowCraft ships none of Blizzard's
//! files). Pathfinding (mmaps) takes about an hour, so it's built in the background at low
//! priority and switched on at the server start after it's done.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::CREATE_NO_WINDOW;

const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
/// The packaged database's port.
pub const DB_PORT: u16 = 3307;
pub const REALM_PORT: u16 = 3724;
pub const WORLD_PORT: u16 = 8085;
/// What a 1.12.1 client's extraction writes: map tiles, building files, assembled vmaps.
const EXPECTED_MAPS: usize = 2429;
const EXPECTED_BUILDINGS: usize = 3913;
const EXPECTED_VMAPS: usize = 6082;
/// Written beside the mmaps once they're all built (an older launcher's one marker).
const MMAPS_DONE: &str = "mmaps/complete.txt";
/// The running build's process (the `cmd` chain), so Stop can end it and its generator.
const MMAPS_PID: &str = "mmaps/builder.pid";

/// Something accepts connections on this local port.
pub fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(150)).is_ok()
}

/// The program listening on a local TCP port (its executable's path), if any: `netstat` for the
/// process id, then its path. Only asked when the port is taken, so the cost doesn't matter.
pub fn port_owner(port: u16) -> Option<String> {
    if !listening(port) {
        return None;
    }
    let out = Command::new("netstat").args(["-ano", "-p", "TCP"]).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pid = text.lines().find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        (cols.len() >= 5 && cols[3] == "LISTENING" && cols[1].ends_with(&format!(":{port}"))).then(|| cols[4].to_string())
    })?;
    let path = Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid}).Path")])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&path.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

fn wait_for(port: u16, secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if listening(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

pub struct Server {
    /// The install's root (server/, mariadb/ beside the launcher).
    pub root: PathBuf,
}

impl Server {
    pub fn bin(&self) -> PathBuf {
        self.root.join("server")
    }
    pub fn data(&self) -> PathBuf {
        self.root.join("server").join("data")
    }
    fn maria(&self) -> PathBuf {
        self.root.join("mariadb")
    }
    pub fn mariadb_client(&self) -> PathBuf {
        self.maria().join("bin").join("mariadb.exe")
    }

    /// Maps and vmaps are there (mmaps are optional).
    pub fn has_maps(&self) -> bool {
        let data = self.data();
        let any = |dir: &Path, ext: &str| {
            std::fs::read_dir(dir).is_ok_and(|mut it| it.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == ext))))
        };
        any(&data.join("maps"), "map") && any(&data.join("vmaps"), "vmtree") && data.join("5875").join("dbc").is_dir()
    }

    pub fn has_mmaps(&self) -> bool {
        let ids = self.map_ids();
        self.data().join(MMAPS_DONE).exists() || (!ids.is_empty() && ids.iter().all(|&id| self.map_done(id).exists()))
    }

    /// The maps the extraction wrote tiles for: a tile file is `<map:3><y:2><x:2>.map`.
    fn map_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = std::fs::read_dir(self.data().join("maps"))
            .map(|d| {
                d.flatten()
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.get(..3)).and_then(|n| n.parse().ok()))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Written once a map's pathfinding is built, so a stopped build resumes with the next map.
    fn map_done(&self, id: u32) -> PathBuf {
        self.data().join("mmaps").join(format!("done-{id:03}.txt"))
    }

    /// Extracts the server's map data from the player's WoW (the folder with WoW.exe in it),
    /// telling `step` how far it is. About ten minutes, once.
    /// `progress` gets 0..1 as the files appear (a 1.12.1 client writes about as many as
    /// [`EXPECTED_MAPS`], [`EXPECTED_BUILDINGS`] and [`EXPECTED_VMAPS`]).
    pub fn extract(&self, wow: &Path, step: &dyn Fn(&str), progress: &dyn Fn(f32)) -> Result<(), String> {
        let data = self.data();
        std::fs::create_dir_all(&data).map_err(|e| format!("couldn't make {}: {e}", data.display()))?;
        let count = |dir: &str| std::fs::read_dir(data.join(dir)).map_or(0, |d| d.count());
        // Each tool's share of the bar (by how long it takes), its folder and how many files it writes.
        let run = |exe: &str, args: &[&str], stdin: Option<&str>, (from, to): (f32, f32), (dir, expected): (&str, usize)| -> Result<(), String> {
            let log = std::fs::File::create(data.join(format!("{exe}.log"))).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
            let mut cmd = Command::new(self.bin().join(format!("{exe}.exe")));
            cmd.args(args).current_dir(&data).stdout(log).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW);
            cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
            let mut child = cmd.spawn().map_err(|e| format!("couldn't start {exe}: {e}"))?;
            if let (Some(text), Some(mut input)) = (stdin, child.stdin.take()) {
                let _ = input.write_all(text.as_bytes());
            }
            let status = loop {
                if let Some(status) = child.try_wait().map_err(|e| format!("{exe}: {e}"))? {
                    break status;
                }
                let done = (count(dir) as f32 / expected as f32).min(1.0);
                progress(from + (to - from) * done);
                std::thread::sleep(Duration::from_millis(500));
            };
            progress(to);
            if status.success() { Ok(()) } else { Err(format!("{exe} failed (see server\\data\\{exe}.log)")) }
        };
        let wow_s = wow.display().to_string();
        let wow_data = wow.join("Data").display().to_string();

        step("Setting up the server: reading maps from your WoW (a few minutes, only the first time)...");
        // Its output path goes in a 128-character buffer: relative, since it runs in `data`.
        run("MapExtractor", &["-i", &wow_s, "-o", ".", "--silent"], None, (0.0, 0.3), ("maps", EXPECTED_MAPS))?;
        let maps = std::fs::read_dir(data.join("maps")).map_or(0, |d| d.flatten().count());
        if maps < 100 {
            return Err("Reading maps from your WoW failed (see server\\data\\MapExtractor.log). Is the WoW folder a 1.12.1 client?".into());
        }
        // The server reads its client tables from <build>/dbc.
        let dbc = data.join("5875").join("dbc");
        if !dbc.is_dir() && data.join("dbc").is_dir() {
            std::fs::create_dir_all(data.join("5875")).map_err(|e| e.to_string())?;
            std::fs::rename(data.join("dbc"), &dbc).map_err(|e| format!("couldn't move dbc: {e}"))?;
        }

        step("Setting up the server: reading buildings from your WoW (only the first time)...");
        run("VMapExtractor", &["-l", "-d", &wow_data, "--silent"], None, (0.3, 0.8), ("Buildings", EXPECTED_BUILDINGS))?;
        step("Setting up the server: assembling buildings...");
        std::fs::create_dir_all(data.join("vmaps")).map_err(|e| e.to_string())?;
        // It waits for Enter before and after.
        run("VMapAssembler", &[], Some("\n\n\n"), (0.8, 1.0), ("vmaps", EXPECTED_VMAPS))?;
        let _ = std::fs::remove_dir_all(data.join("Buildings"));
        if !self.has_maps() {
            return Err("Map extraction didn't produce the server's maps. Is the WoW folder a 1.12.1 client?".into());
        }
        Ok(())
    }

    /// Builds the mmaps (creature pathfinding) in the background while WowCraft runs, one map at a
    /// time at idle priority on a quarter of the cores, unless they're done or already building.
    /// [`Self::stop_mmaps`] ends it (Stop, the game closing, the launcher closing); the next start
    /// resumes with the first map not yet done. They're used from the next server start.
    pub fn build_mmaps_in_background(&self, running: &[String]) {
        if self.has_mmaps() || running.iter().any(|p| p == "movemapgenerator.exe") {
            return;
        }
        let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).max(1));
        let data = self.data();
        let _ = std::fs::create_dir_all(data.join("mmaps"));
        let generator = self.bin().join("MoveMapGenerator.exe");
        let steps: Vec<String> = self
            .map_ids()
            .into_iter()
            .filter(|&id| !self.map_done(id).exists())
            .map(|id| {
                format!(
                    "\"{}\" {id} --silent --threads {threads} --skipJunkMaps --skipBattlegrounds --offMeshInput offmesh.txt --configInputPath config.json >> mmapgen.log 2>&1 && echo done> \"{}\"",
                    generator.display(),
                    self.map_done(id).display()
                )
            })
            .collect();
        if steps.is_empty() {
            return;
        }
        // A batch file, a map a line: all of them on one command line is past Windows' limit.
        let script = data.join("mmaps").join("build.cmd");
        if std::fs::write(&script, format!("@echo off\r\n{}\r\n", steps.join("\r\n"))).is_err() {
            return;
        }
        let child = Command::new("cmd")
            .arg("/c")
            .arg(&script)
            .current_dir(&data)
            .creation_flags(CREATE_NO_WINDOW | IDLE_PRIORITY_CLASS)
            .spawn();
        if let Ok(child) = child {
            let _ = std::fs::write(data.join(MMAPS_PID), child.id().to_string());
        }
    }

    /// Ends a background mmaps build: its chain (so no next map starts) and the generator.
    pub fn stop_mmaps(&self) {
        let pid_file = self.data().join(MMAPS_PID);
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|p| p.trim().parse::<u32>().ok()) {
            let _ = Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
        let _ = std::fs::remove_file(pid_file);
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "MoveMapGenerator.exe"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }

    /// Points the server's configs at this folder and the packaged database.
    pub fn write_configs(&self) -> Result<(), String> {
        let slash = |p: PathBuf| p.display().to_string().replace('\\', "/");
        let db = |name: &str| format!("\"127.0.0.1;{DB_PORT};mangos;mangos;{name}\"");
        let mmap = if self.has_mmaps() { "1" } else { "0" };
        let mangosd: Vec<(&str, String)> = vec![
            ("DataDir", format!("\"{}\"", slash(self.data()))),
            ("LogsDir", format!("\"{}\"", slash(self.root.join("server").join("logs")))),
            ("LoginDatabase.Info", db("realmd")),
            ("WorldDatabase.Info", db("mangos")),
            ("CharacterDatabase.Info", db("characters")),
            ("LogsDatabase.Info", db("logs")),
            ("mmap.enabled", mmap.into()),
            // No window, so no console: its stdin is at end-of-file, which it takes for "shutdown".
            ("Console.Enable", "0".into()),
            // Only this PC connects (friends come through Minecraft's tunnel): no firewall prompt,
            // nothing open to the network.
            ("BindIP", "\"127.0.0.1\"".into()),
            ("BeepAtStart", "0".into()),
            // One or two players: raids (and their quests) without a raid group.
            ("Instance.IgnoreRaid", "1".into()),
            ("Quests.IgnoreRaid", "1".into()),
        ];
        let realmd: Vec<(&str, String)> = vec![
            ("LoginDatabaseInfo", db("realmd")),
            ("BindIP", "\"127.0.0.1\"".into()),
            ("LogsDir", format!("\"{}\"", slash(self.root.join("server").join("logs")))),
        ];
        let _ = std::fs::create_dir_all(self.root.join("server").join("logs"));
        set_keys(&self.bin().join("mangosd.conf"), &mangosd)?;
        set_keys(&self.bin().join("realmd.conf"), &realmd)
    }

    /// The database, the login server and the world server, each unless it's up.
    pub fn start(&self, step: &dyn Fn(&str)) -> Result<(), String> {
        // A server port already open is taken for ours (from an earlier start) only if it is: a
        // development or second WoW server on the login port had our game log in to it, where its
        // account didn't exist ("the information you have entered is not valid").
        let root = self.root.display().to_string().to_lowercase();
        for (port, what) in [(DB_PORT, "database"), (REALM_PORT, "login server"), (WORLD_PORT, "world server")] {
            if let Some(path) = port_owner(port).filter(|path| !path.to_lowercase().starts_with(&root)) {
                return Err(format!(
                    "Another program holds the {what}'s port ({port}): {path}. Close it (another WoW server?) and press Play again."
                ));
            }
        }
        // A new install: its database from the empty one shipped beside (an update never brings
        // a `data` folder, so unzipping over an install keeps its characters).
        let data = self.maria().join("data");
        let clean = self.maria().join("data-clean");
        if !data.exists() && clean.exists() {
            step("Preparing the database...");
            copy_dir(&clean, &data).map_err(|e| format!("couldn't make the database: {e}"))?;
        }
        if !listening(DB_PORT) {
            step("Starting the database...");
            let maria = self.maria();
            Command::new(maria.join("bin").join("mariadbd.exe"))
                .arg("--no-defaults")
                .arg(format!("--basedir={}", maria.display()))
                .arg(format!("--datadir={}", maria.join("data").display()))
                .arg(format!("--port={DB_PORT}"))
                .arg("--bind-address=127.0.0.1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("couldn't start the database: {e}"))?;
            if !wait_for(DB_PORT, 40) {
                return Err("The database didn't start (see mariadb\\data\\*.err).".into());
            }
        }
        self.write_configs()?;
        if !listening(WORLD_PORT) {
            // With the world server down, so it reads them fresh and holds no item numbers.
            self.bigger_bags();
        }
        if !listening(REALM_PORT) {
            step("Starting the login server...");
            Command::new(self.bin().join("realmd.exe"))
                .current_dir(self.bin())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("couldn't start the login server: {e}"))?;
        }
        if !listening(WORLD_PORT) {
            step("Starting the world server (up to a minute)...");
            Command::new(self.bin().join("mangosd.exe"))
                .current_dir(self.bin())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .map_err(|e| format!("couldn't start the world server: {e}"))?;
            if !wait_for(WORLD_PORT, 120) {
                return Err("The world server didn't start (see server\\logs).".into());
            }
        }
        if !wait_for(REALM_PORT, 20) {
            return Err("The login server didn't start (see server\\logs).".into());
        }
        Ok(())
    }

    /// Bigger bags from the start: a new character starts with four Traveler's Backpacks (16
    /// slots) in its bag slots, and one made before gets them once, in whichever bag slots are
    /// empty. Best effort: a failure leaves the bags as they were.
    fn bigger_bags(&self) {
        const SQL: &str = "            DELETE FROM mangos.playercreateinfo_item WHERE itemid = 4500;             INSERT INTO mangos.playercreateinfo_item (race, class, itemid, amount)                 SELECT DISTINCT race, class, 4500, 4 FROM mangos.playercreateinfo;             CREATE TABLE IF NOT EXISTS characters.wowcraft_bags (guid INT UNSIGNED NOT NULL PRIMARY KEY);             SET @g := (SELECT IFNULL(MAX(guid), 0) FROM characters.item_instance);             BAGS;             INSERT IGNORE INTO characters.wowcraft_bags (guid) SELECT guid FROM characters.characters;";
        let slot = |s: u32| {
            format!(
                "SET @from := @g;                  INSERT INTO characters.item_instance (guid, item_id, owner_guid, count, charges, enchantments)                      SELECT (@g := @g + 1), 4500, c.guid, 1, '0 0 0 0 0 ', '0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 ' FROM characters.characters c                      WHERE c.guid NOT IN (SELECT guid FROM characters.wowcraft_bags)                      AND NOT EXISTS (SELECT 1 FROM characters.character_inventory i WHERE i.guid = c.guid AND i.bag = 0 AND i.slot = {s});                  INSERT INTO characters.character_inventory (guid, bag, slot, item_guid, item_id)                      SELECT owner_guid, 0, {s}, guid, 4500 FROM characters.item_instance WHERE guid > @from"
            )
        };
        // The four bag slots (INVENTORY_SLOT_BAG_START 19 to 22).
        let sql = SQL.replace("BAGS", &(19..=22).map(slot).collect::<Vec<_>>().join("; "));
        let ok = Command::new(self.mariadb_client())
            .args(["-h", "127.0.0.1", "-P", &DB_PORT.to_string(), "-u", "root", "-e", &sql])
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("WowCraft: couldn't set up the bigger bags");
        }
    }

    /// The world server first (it saves characters on the way out), then the rest.
    pub fn stop(&self) {
        let kill = |image: &str, force: bool| {
            let mut args = vec!["/IM", image];
            if force {
                args.push("/F");
            }
            let _ = Command::new("taskkill").args(args).creation_flags(CREATE_NO_WINDOW).status();
        };
        kill("mangosd.exe", false);
        let start = Instant::now();
        while listening(WORLD_PORT) && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(500));
        }
        kill("mangosd.exe", true);
        kill("realmd.exe", true);
        let _ = Command::new(self.maria().join("bin").join("mariadb-admin.exe"))
            .args(["-h", "127.0.0.1", "-P", &DB_PORT.to_string(), "-u", "root", "shutdown"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

/// Sets `key = value` lines in a mangos-style config, keeping everything else.
fn set_keys(path: &Path, keys: &[(&str, String)]) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let out: Vec<String> = text
        .lines()
        .map(|line| {
            let name = line.split('=').next().unwrap_or("").trim();
            match keys.iter().find(|(k, _)| *k == name && !line.trim_start().starts_with('#')) {
                Some((k, v)) => format!("{k} = {v}"),
                None => line.to_string(),
            }
        })
        .collect();
    std::fs::write(path, out.join("\r\n") + "\r\n").map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keys_are_replaced_and_the_rest_kept() {
        let path = std::env::temp_dir().join(format!("wowcraft-conf-{}.conf", std::process::id()));
        std::fs::write(&path, "# DataDir = \"old\"\nDataDir = \"C:/old\"\nOther = 1\nmmap.enabled = 1\n").unwrap();
        set_keys(&path, &[("DataDir", "\"D:/new\"".into()), ("mmap.enabled", "0".into())]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "# DataDir = \"old\"\r\nDataDir = \"D:/new\"\r\nOther = 1\r\nmmap.enabled = 0\r\n");
        let _ = std::fs::remove_file(path);
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let path = e.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(e.file_name()))?;
        }
    }
    Ok(())
}
