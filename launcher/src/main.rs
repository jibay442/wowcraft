//! WowCraft launcher: one window to play, host co-op or join a friend.
//!
//! Play starts the WoW server (if it isn't running), WoW and the hidden Minecraft, in that order.
//! Host co-op does the same and opens the Minecraft world to friends, showing the join link to
//! copy. Join takes a friend's link: Minecraft joins their world, their server makes our WoW
//! account, and WoW logs in through the Minecraft connection, so nothing else is needed.
//!
//! Two installs:
//! - the packaged one (`WowCraft.exe` beside `wow/benilla.exe`, `server/`, `mariadb/`,
//!   `minecraft/WowCraft`, `prism/`): the server is set up on first Play from the player's own
//!   WoW ([`setup`]), and Minecraft runs through Prism Launcher with the player's own account: an
//!   installed Prism if there is one, else the bundled portable copy. The instance is copied into
//!   that Prism on each start and launched with `--launch`; settings reach the mod through
//!   `config/skycraft.properties`, since Prism doesn't pass our environment on;
//! - the development checkout (the Benilla checkout, `server/Release`, the Fabric dev client).
//!
//! On Linux the launcher is an AppImage put in the unzipped WowCraft folder. It runs the install's
//! Linux programs where it has them and its Windows ones through Wine ([`platform`]); Minecraft
//! runs through the bundled (Windows) Prism Launcher under Wine too, in the same Wine prefix: the
//! mod and WoW share Windows shared memory, which only works between programs of one prefix.

#![windows_subsystem = "windows"]

mod platform;
mod setup;
mod update;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

use platform::Program;

/// Where everything lives, detected once and kept in `wowcraft-launcher.cfg` beside the install.
#[derive(Clone)]
struct Paths {
    root: PathBuf,
    /// The packaged install (else the development checkout).
    packaged: bool,
    wow_data: PathBuf,
    skin: String,
    name: String,
    /// Prism Launcher's exe (set in the cfg, installed, or the bundled one).
    prism: Option<PathBuf>,
}

/// The Prism instance the packaged install sets up and launches.
const INSTANCE: &str = "WowCraft";

impl Paths {
    fn detect() -> Self {
        // An AppImage runs from a mount of its own: its file's folder is the install.
        let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok());
        let exe_dir = exe.and_then(|e| e.parent().map(Path::to_path_buf));
        let packaged_root = exe_dir.clone().filter(|d| benilla_in(d).exists());
        // Else the folder chosen in Settings.
        let known = read_cfg(&chosen_root_file()).remove("root").map(PathBuf::from).filter(|d| benilla_in(d).exists());
        let root = std::env::var_os("WOWCRAFT_ROOT")
            .map(PathBuf::from)
            .or_else(|| packaged_root.clone())
            .or(known)
            .or_else(|| exe_dir.as_deref()?.ancestors().find(|p| p.join("benilla").is_dir() && p.join("server").is_dir()).map(Path::to_path_buf))
            // Neither: the folder the launcher is in (its problems list says what's missing).
            .or(exe_dir)
            .unwrap_or_default();
        let packaged = benilla_in(&root).exists();
        // Nothing guessed: the WoW folder is the one chosen in Settings.
        let mut paths = Paths {
            wow_data: PathBuf::new(),
            skin: String::new(),
            name: String::new(),
            prism: None,
            packaged,
            root,
        };
        for (k, v) in read_cfg(&paths.cfg_file()) {
            match k.as_str() {
                "wow_data" => paths.wow_data = PathBuf::from(v),
                "skin" => paths.skin = v,
                "name" => paths.name = v,
                "prism" if !v.is_empty() => paths.prism = Some(PathBuf::from(v)),
                _ => {}
            }
        }
        // A folder given for Prism (typed in Settings) means the program in it: Windows answers
        // "access denied" when asked to run a folder.
        paths.prism = paths.prism.map(prism_exe);
        if cfg!(not(windows)) {
            // Linux: only the bundled Windows Prism will do (see the module's notes).
            paths.prism = Some(bundled_prism(&paths.root)).filter(|p| p.exists());
        } else if paths.prism.as_ref().is_none_or(|p| !p.is_file()) {
            paths.prism = find_prism().or_else(|| Some(bundled_prism(&paths.root)).filter(|p| p.exists()));
        }
        paths
    }

    /// The WoW folder or its Data folder, whichever was given: the Data folder.
    fn data_dir(&self) -> PathBuf {
        let inner = self.wow_data.join("Data");
        if inner.is_dir() { inner } else { self.wow_data.clone() }
    }
    /// The WoW folder itself (the one with Data in it).
    fn wow_dir(&self) -> PathBuf {
        let data = self.data_dir();
        data.parent().map(Path::to_path_buf).unwrap_or(data)
    }

    /// The development checkout (else the launcher found no WowCraft folder at all).
    fn dev(&self) -> bool {
        !self.packaged && self.root.join("benilla").is_dir()
    }

    fn cfg_file(&self) -> PathBuf {
        self.root.join("wowcraft-launcher.cfg")
    }

    fn save(&self) {
        // The bundled Prism isn't remembered: an install made later takes over.
        let bundled = bundled_prism(&self.root);
        let prism = self.prism.as_ref().filter(|p| **p != bundled).map(|p| p.display().to_string()).unwrap_or_default();
        let text = format!("wow_data={}\nskin={}\nname={}\nprism={}\n", self.wow_data.display(), self.skin, self.name, prism);
        let _ = std::fs::write(self.cfg_file(), text);
    }

    fn server(&self) -> setup::Server {
        setup::Server { root: self.root.clone() }
    }
    /// The development checkout's server folder.
    fn dev_server(&self) -> PathBuf {
        self.root.join("server").join("Release")
    }
    fn can_host(&self) -> bool {
        if self.packaged { self.server().program("mangosd").exists() } else { platform::program(&self.dev_server(), "mangosd", &[]).exists() }
    }
    /// Where WoW runs (its settings and logs land here).
    fn benilla(&self) -> PathBuf {
        if self.packaged { self.root.join("wow") } else { self.root.join("benilla") }
    }
    /// WoW: our Benilla client (a Linux build of it, or the Windows one through Wine).
    fn benilla_exe(&self) -> Program {
        let dir = if self.packaged { self.root.join("wow") } else { self.root.join("benilla/target/release") };
        platform::program(&dir, "benilla", &[])
    }
    fn fabric(&self) -> PathBuf {
        self.root.join("wowcraft").join("fabric")
    }
    /// Prism's data folder: beside its exe when portable, else in AppData.
    fn prism_data(&self) -> Option<PathBuf> {
        let exe = self.prism.as_ref()?;
        let dir = exe.parent()?.to_path_buf();
        if dir.join("portable.txt").exists() {
            return Some(dir);
        }
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("PrismLauncher"))
    }
    fn instance(&self) -> Option<PathBuf> {
        self.prism_data().map(|d| d.join("instances").join(INSTANCE))
    }
    /// Minecraft's game folder (its config, the co-op files).
    fn game_dir(&self) -> PathBuf {
        if self.packaged {
            self.instance().map(|i| i.join("minecraft")).unwrap_or_else(|| self.root.join("minecraft"))
        } else {
            self.fabric().join("run")
        }
    }
    fn link_file(&self) -> PathBuf {
        self.game_dir().join("wowcraft-coop-link.txt")
    }
    fn account_file(&self) -> PathBuf {
        self.game_dir().join("wowcraft-coop-account.properties")
    }

    /// Copies (or refreshes) the bundled instance into Prism: its settings and our mods; the
    /// player's worlds, options and any mods of their own stay.
    fn install_instance(&self) -> Result<(), String> {
        let src = self.root.join("minecraft").join(INSTANCE);
        let dst = self.instance().ok_or("Prism Launcher not found")?;
        let mods = dst.join("minecraft").join("mods");
        std::fs::create_dir_all(&mods).map_err(|e| format!("couldn't make the Minecraft instance: {e}"))?;
        for name in ["instance.cfg", "mmc-pack.json"] {
            std::fs::copy(src.join(name), dst.join(name)).map_err(|e| format!("couldn't copy {name}: {e}"))?;
        }
        let ours: Vec<PathBuf> = std::fs::read_dir(src.join("minecraft").join("mods"))
            .map_err(|e| format!("bundled mods missing: {e}"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        // Older copies of our mods go (a version bump renames the jar).
        let stem = |p: &Path| {
            p.file_name().and_then(|n| n.to_str()).and_then(|n| n.split(['-', '+']).next()).unwrap_or_default().to_lowercase()
        };
        if let Ok(old) = std::fs::read_dir(&mods) {
            for e in old.flatten() {
                let p = e.path();
                if ours.iter().any(|o| stem(o) == stem(&p) && o.file_name() != p.file_name()) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        for jar in &ours {
            if let Some(name) = jar.file_name() {
                std::fs::copy(jar, mods.join(name)).map_err(|e| format!("couldn't copy a mod: {e}"))?;
            }
        }
        // Settings our mods need (Lithium's collision code off: it skips our WoW ground).
        if let Ok(configs) = std::fs::read_dir(src.join("minecraft").join("config")) {
            let config = dst.join("minecraft").join("config");
            std::fs::create_dir_all(&config).map_err(|e| format!("couldn't make the config folder: {e}"))?;
            for file in configs.flatten() {
                std::fs::copy(file.path(), config.join(file.file_name())).map_err(|e| format!("couldn't copy a setting: {e}"))?;
            }
        }
        Ok(())
    }

    /// What's missing for the launcher to work, in words.
    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(problem) = self.wow_problem() {
            out.push(problem);
        }
        let benilla = self.benilla_exe();
        if !benilla.exists() {
            out.push(if self.packaged {
                format!("{} is missing: unzip the whole folder", Path::new("wow").join("benilla.exe").display())
            } else if self.dev() {
                "benilla not built".into()
            } else {
                "WowCraft's folder wasn't found: choose your unzipped WowCraft folder (with wow, server and minecraft in it) in Settings".into()
            });
        } else if benilla.wine && platform::wine().is_none() {
            out.push("Wine is needed to run WowCraft's Windows programs: install it (your distribution's wine package)".into());
        }
        if self.packaged && self.prism.is_none() {
            out.push(if cfg!(windows) {
                "Prism Launcher not found: unzip the whole folder, or install it (prismlauncher.org)".into()
            } else {
                "prism/prismlauncher.exe is missing: unzip the whole folder".into()
            });
        }
        out
    }

    /// What's wrong with the chosen WoW folder, if anything: missing, or another version of WoW
    /// (its files don't fit the 1.12 server: it stopped at start with "wrong client version DBC").
    fn wow_problem(&self) -> Option<String> {
        if self.wow_data.as_os_str().is_empty() {
            return Some("Choose your WoW 1.12 folder (the one with WoW.exe and Data in it) in Settings".into());
        }
        let data = self.data_dir();
        let shown = self.wow_data.display();
        // Today's WoW (Retail, Classic Era, Classic): CASC storage, no MPQ archives.
        let casc = [self.wow_data.join(".build.info"), self.wow_data.join("..").join(".build.info"), data.join("data")]
            .iter()
            .any(|p| p.exists());
        if casc && !data.join("dbc.MPQ").exists() {
            return Some(format!(
                "{shown} is today's WoW (Retail or Classic). WowCraft needs the original 1.12.1 client from 2006 (choose its folder in Settings)"
            ));
        }
        // The Burning Crusade and Wrath of the Lich King: MPQs, but their own.
        if ["common.MPQ", "expansion.MPQ", "lichking.MPQ", "common-2.MPQ"].iter().any(|f| data.join(f).exists()) {
            return Some(format!("{shown} is a later WoW (The Burning Crusade or Wrath). WowCraft needs the 1.12.1 client (choose its folder in Settings)"));
        }
        if !data.join("dbc.MPQ").exists() {
            return Some(format!("WoW 1.12 not found in {shown} (choose the folder with WoW.exe and Data in Settings)"));
        }
        None
    }
}

/// Where the WowCraft folder chosen in Settings is kept (for a launcher that isn't in it).
fn chosen_root_file() -> PathBuf {
    let config = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default()
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| platform::home().join(".config"))
    };
    config.join("WowCraft").join("launcher.cfg")
}

/// WoW's program in a packaged install's root, either build.
fn benilla_in(root: &Path) -> PathBuf {
    let native = root.join("wow").join("benilla");
    if cfg!(not(windows)) && native.is_file() { native } else { root.join("wow").join("benilla.exe") }
}

/// The Prism Launcher bundled in the install's `prism` folder.
fn bundled_prism(root: &Path) -> PathBuf {
    root.join("prism").join("prismlauncher.exe")
}

/// Prism's program from what was chosen: a folder means the prismlauncher.exe in it.
fn prism_exe(chosen: PathBuf) -> PathBuf {
    if chosen.is_dir() { chosen.join("prismlauncher.exe") } else { chosen }
}

/// How Prism runs: itself, or (Linux) through Wine, in WoW's prefix.
fn prism_program(exe: &Path) -> Program {
    Program { path: exe.to_path_buf(), wine: cfg!(not(windows)), bundled: true }
}

/// Prism Launcher in the places its installers put it.
fn find_prism() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    [
        env("LOCALAPPDATA").map(|d| d.join("Programs/PrismLauncher/prismlauncher.exe")),
        env("ProgramFiles").map(|d| d.join("PrismLauncher/prismlauncher.exe")),
        env("ProgramFiles(x86)").map(|d| d.join("PrismLauncher/prismlauncher.exe")),
        env("USERPROFILE").map(|d| d.join("scoop/apps/prismlauncher/current/prismlauncher.exe")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.exists())
}

fn read_cfg(path: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

/// Which processes are up, by name, lowercased and without `.exe` (refreshed every couple of seconds).
fn running() -> Vec<String> {
    platform::running()
}

/// Sets `key=value` lines in a properties file, keeping the rest.
fn set_properties(path: &Path, keys: &[(&str, String)]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !keys.iter().any(|(k, _)| l.split('=').next().is_some_and(|n| n.trim() == *k)))
        .map(str::to_string)
        .collect();
    for (k, v) in keys {
        lines.push(format!("{k}={}", v.replace('\\', "\\\\")));
    }
    let _ = std::fs::write(path, lines.join("\n") + "\n");
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Solo,
    Host,
    Join,
}

/// The launcher's shared state, written by the start-up thread and read by the window.
#[derive(Default)]
struct Shared {
    step: String,
    /// The first-time setup's progress, 0..1, while it runs.
    progress: Option<f32>,
    error: Option<String>,
    minecraft: Option<Child>,
    busy: bool,
    /// A newer WowCraft on GitHub, found at start.
    update: Option<update::Release>,
}

struct App {
    paths: Paths,
    shared: Arc<Mutex<Shared>>,
    join_link: String,
    mode: Option<Mode>,
    procs: Vec<String>,
    /// The packaged server's ports answer (polled with the processes).
    server_up: bool,
    polled: Instant,
    show_settings: bool,
    /// Stop takes the WoW server down too.
    stop_server: bool,
    copied: Option<Instant>,
    /// The join link is shown (it's dots until asked: a streamer's viewers would join too).
    show_link: bool,
    /// The window's height, as last set.
    height: f32,
}

const GOLD: Color32 = Color32::from_rgb(255, 205, 80);
const INK: Color32 = Color32::from_rgb(28, 22, 10);
const MUTED: Color32 = Color32::from_gray(150);
const ERROR: Color32 = Color32::from_rgb(255, 120, 100);

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(22, 22, 26);
        visuals.extreme_bg_color = Color32::from_rgb(14, 14, 17);
        let round = egui::CornerRadius::same(6);
        for w in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            w.corner_radius = round;
        }
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 44, 52);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 58, 68);
        visuals.selection.bg_fill = Color32::from_rgb(150, 115, 40);
        cc.egui_ctx.set_visuals(visuals);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(17.0));
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
        style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.5));
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        cc.egui_ctx.set_style(style);
        let mut app = App {
            paths: Paths::detect(),
            shared: Arc::default(),
            join_link: String::new(),
            mode: None,
            procs: Vec::new(),
            server_up: false,
            polled: Instant::now(),
            // WOWCRAFT_LAUNCHER_PREVIEW=settings opens on that page (for checking the layout).
            show_settings: std::env::var("WOWCRAFT_LAUNCHER_PREVIEW").is_ok_and(|v| v == "settings"),
            stop_server: false,
            copied: None,
            show_link: false,
            height: 0.0,
        };
        app.poll();
        if app.paths.packaged {
            // After an update: the old launcher and the download go. Then: is there a newer one?
            update::clean_up(&app.paths.root);
            let shared = app.shared.clone();
            std::thread::spawn(move || {
                let found = update::check();
                shared.lock().unwrap().update = found;
            });
        }
        // The game closed however it closed (Alt+F4, a crash, its own Exit), the window shown or
        // not (a minimized window isn't updated, so its own check waits): once WoW and Minecraft
        // are both gone, the server goes too, after a moment for it to save the character's logout.
        if app.paths.packaged {
            let (paths, shared) = (app.paths.clone(), app.shared.clone());
            std::thread::spawn(move || {
                let mut seen = false;
                let mut gone_since: Option<Instant> = None;
                loop {
                    std::thread::sleep(Duration::from_secs(3));
                    if shared.lock().unwrap().busy {
                        gone_since = None;
                        continue;
                    }
                    let wow = running().iter().any(|p| p == "benilla");
                    if wow || minecraft_running(&paths.game_dir()) {
                        seen = true;
                        gone_since = None;
                        continue;
                    }
                    let server = setup::listening(setup::REALM_PORT) || setup::listening(setup::WORLD_PORT) || setup::listening(setup::DB_PORT);
                    if !seen || !server {
                        continue;
                    }
                    if gone_since.get_or_insert_with(Instant::now).elapsed() >= Duration::from_secs(10) {
                        stop_all(&paths, &shared, true);
                        shared.lock().unwrap().step = "The game closed, so the server stopped.".into();
                        seen = false;
                        gone_since = None;
                    }
                }
            });
        }
        // WOWCRAFT_LAUNCHER_PREVIEW=busy shows the start-up screen mid-setup (for checking the layout).
        if std::env::var("WOWCRAFT_LAUNCHER_PREVIEW").is_ok_and(|v| v == "busy") {
            app.mode = Some(Mode::Host);
            let mut s = app.shared.lock().unwrap();
            s.busy = true;
            s.step = "Setting up the server: reading buildings from your WoW (only the first time)...".into();
            s.progress = Some(0.46);
        }
        app
    }

    /// Downloads the newer WowCraft, stops the server, puts the new programs in place and restarts
    /// the launcher; saves and settings stay.
    fn start_update(&mut self, release: update::Release) {
        let paths = self.paths.clone();
        let shared = self.shared.clone();
        {
            let mut s = shared.lock().unwrap();
            s.busy = true;
            s.error = None;
            s.update = None;
            s.step = format!("Downloading WowCraft {}...", release.version);
            s.progress = Some(0.0);
        }
        std::thread::spawn(move || {
            let result = (|| {
                let new = update::download(&paths.root, &release, &|p| shared.lock().unwrap().progress = Some(p))?;
                {
                    let mut s = shared.lock().unwrap();
                    s.progress = None;
                    s.step = "Installing the update...".into();
                }
                // Its database and server hold files the update replaces.
                paths.server().stop();
                update::apply(&paths.root, &new)?;
                update::restart(&paths.root)
            })();
            match result {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    let mut s = shared.lock().unwrap();
                    s.busy = false;
                    s.progress = None;
                    s.step.clear();
                    s.error = Some(e);
                }
            }
        });
    }

    fn poll(&mut self) {
        self.procs = running();
        self.server_up = if self.paths.packaged {
            setup::listening(setup::REALM_PORT) && setup::listening(setup::WORLD_PORT)
        } else {
            self.up("mangosd") && self.up("realmd")
        };
        self.polled = Instant::now();
    }

    fn up(&self, image: &str) -> bool {
        self.procs.iter().any(|p| p == image)
    }

    fn minecraft_up(&self) -> bool {
        if self.paths.packaged {
            minecraft_running(&self.paths.game_dir())
        } else {
            self.shared.lock().unwrap().minecraft.as_mut().is_some_and(|c| c.try_wait().ok().flatten().is_none())
        }
    }

    fn start(&mut self, mode: Mode) {
        let paths = self.paths.clone();
        let shared = self.shared.clone();
        let link = self.join_link.trim().trim_start_matches("https://").trim_end_matches('/').to_string();
        self.mode = Some(mode);
        {
            let mut s = shared.lock().unwrap();
            s.busy = true;
            s.error = None;
        }
        std::thread::spawn(move || {
            let result = launch(&paths, mode, &link, &shared);
            let mut s = shared.lock().unwrap();
            s.busy = false;
            if let Err(e) = result {
                s.error = Some(e);
                s.step.clear();
            }
        });
    }

    /// Closes WoW and Minecraft (and the server too, if asked), off the window's thread.
    fn stop(&mut self, server_too: bool) {
        self.mode = None;
        self.poll();
        let wow = self.up("benilla");
        let minecraft = self.minecraft_up();
        let server = server_too && (self.server_up || self.up("mariadbd"));
        let shared = self.shared.clone();
        if !wow && !minecraft && !server {
            let mut s = shared.lock().unwrap();
            s.error = None;
            s.step = "Nothing is running.".into();
            return;
        }
        let paths = self.paths.clone();
        {
            let mut s = shared.lock().unwrap();
            s.busy = true;
            s.error = None;
            s.step = if minecraft { "Stopping (Minecraft saves first)...".into() } else { "Stopping...".into() };
        }
        std::thread::spawn(move || {
            stop_all(&paths, &shared, server);
            let mut s = shared.lock().unwrap();
            s.busy = false;
            s.step = if server { "Stopped everything.".into() } else { "Stopped.".into() };
        });
    }
}

fn stop_all(paths: &Paths, shared: &Arc<Mutex<Shared>>, server_too: bool) {
    platform::kill("benilla", true);
    // Minecraft saves the world and quits when asked (wowcraft-quit.request; Prism's also by itself
    // once WoW has gone); the dev client is forced only if it hasn't within 20 seconds.
    let _ = std::fs::write(paths.game_dir().join("wowcraft-quit.request"), "");
    let child = shared.lock().unwrap().minecraft.take();
    if let Some(mut mc) = child {
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_secs(20) && mc.try_wait().ok().flatten().is_none() {
            std::thread::sleep(Duration::from_millis(250));
        }
        platform::kill_tree(mc.id());
        let _ = mc.wait();
    } else if paths.packaged {
        // Give Prism's Minecraft its time to save before the server goes.
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_secs(20) && minecraft_running(&paths.game_dir()) {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    if paths.packaged {
        paths.server().stop_mmaps();
    }
    if server_too {
        if paths.packaged {
            paths.server().stop();
        } else {
            // The world server first: it saves characters on the way out.
            platform::kill("mangosd", false);
            std::thread::sleep(Duration::from_secs(3));
            platform::kill("realmd", true);
            let admin = platform::program(&paths.root.join("mariadb/bin"), "mariadb-admin", &["mariadb-admin", "mysqladmin"]);
            let _ = admin.command().args(["-u", "root", "shutdown"]).status();
        }
    }
}

fn step(shared: &Arc<Mutex<Shared>>, text: &str) {
    shared.lock().unwrap().step = text.to_string();
}

/// The development checkout's server, as play-wowcraft.ps1 starts it.
fn start_dev_server(p: &Paths, shared: &Arc<Mutex<Shared>>) -> Result<(), String> {
    let procs = running();
    let up = |image: &str| procs.iter().any(|x| x == image);
    if !up("mariadbd") {
        step(shared, "Starting the database...");
        let maria = p.root.join("mariadb");
        let server = platform::program(&maria.join("bin"), "mariadbd", &["mariadbd", "mysqld"]);
        server
            .command()
            .arg(format!("--defaults-file={}", server.path_arg(&maria.join("data/my.ini"))))
            .spawn()
            .map_err(|e| format!("couldn't start the database: {e}"))?;
        std::thread::sleep(Duration::from_secs(5));
    }
    if !up("realmd") {
        step(shared, "Starting the login server...");
        platform::program(&p.dev_server(), "realmd", &[])
            .command()
            .current_dir(p.dev_server())
            .spawn()
            .map_err(|e| format!("couldn't start the login server: {e}"))?;
    }
    if !up("mangosd") {
        step(shared, "Starting the world server (takes a few seconds)...");
        platform::program(&p.dev_server(), "mangosd", &[])
            .command()
            .current_dir(p.dev_server())
            .spawn()
            .map_err(|e| format!("couldn't start the world server: {e}"))?;
        std::thread::sleep(Duration::from_secs(15));
    }
    Ok(())
}

/// Prism starts Minecraft with the player's own account (it asks them to sign in if needed). If
/// the chosen Prism won't start (an antivirus blocking it, say), the bundled one does, with its
/// own copy of the instance.
fn start_prism(p: &Paths) -> Result<(), String> {
    let prism = p.prism.as_ref().ok_or("Prism Launcher not found")?;
    let start = |exe: &Path| prism_program(exe).command().args(["--launch", INSTANCE]).spawn();
    let bundled = bundled_prism(&p.root);
    let started = start(prism).or_else(|e| {
        // Another Prism that won't start: the bundled one instead.
        if bundled.is_file() && bundled != *prism {
            // Its own copy of the instance first: the other Prism's folder had it.
            let mut own = p.clone();
            own.prism = Some(bundled.clone());
            own.install_instance().map_err(|err| std::io::Error::other(err))?;
            start(&bundled)
        } else {
            Err(e)
        }
    });
    if let Err(e) = started {
        return Err(if cfg!(windows) && e.raw_os_error() == Some(5) {
            format!(
                "Windows refused to start Prism Launcher ({}). Your antivirus may be blocking it: allow that file, or right-click it, Properties, Unblock. Then press PLAY again.",
                prism.display()
            )
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Prism Launcher ({}) isn't allowed to run: make it executable (chmod +x), then press PLAY again.", prism.display())
        } else {
            format!("couldn't start Prism Launcher ({}): {e}", prism.display())
        });
    }
    Ok(())
}

/// What Minecraft reads at start, in config/skycraft.properties: whose world, and (packaged) the
/// co-op and account settings the environment can't carry through Prism.
fn minecraft_settings(p: &Paths, mode: Mode, link: &str) {
    let props = p.game_dir().join("config/skycraft.properties");
    let join = if mode == Mode::Join { link.to_string() } else { String::new() };
    let mut keys: Vec<(&str, String)> = vec![("join", join)];
    if p.packaged {
        let host = mode == Mode::Host;
        keys.extend([
            ("lan_port", if host { "25566".to_string() } else { String::new() }),
            ("host_account", "1".into()),
            ("mariadb", minecraft_mariadb(p)),
            ("mariadb_port", setup::DB_PORT.to_string()),
        ]);
    }
    set_properties(&props, &keys);
    if mode == Mode::Host {
        let _ = std::fs::remove_file(p.link_file());
    }
    let _ = std::fs::remove_file(p.game_dir().join("wowcraft-quit.request"));
}

/// The database client as Minecraft's mod runs it: on Linux the mod runs under Wine, so the
/// bundled Windows client, by its Windows path.
fn minecraft_mariadb(p: &Paths) -> String {
    if cfg!(windows) {
        return p.server().mariadb_client().display().to_string();
    }
    platform::windows_path(&p.root.join("mariadb").join("bin").join("mariadb.exe"))
}

/// Whether our Minecraft is running: it holds its log open. Any Java counted before, so a build
/// tool or another Java game kept "Minecraft" up: the server wasn't stopped when the game closed,
/// and closing the launcher waited out its 20 seconds.
fn minecraft_running(game_dir: &Path) -> bool {
    platform::held_open(&game_dir.join("logs").join("latest.log"))
}

/// The process ids of the Javas running now (Minecraft is one).
fn java_pids() -> Vec<u32> {
    platform::processes().into_iter().filter(|(_, image)| image == "javaw" || image == "java").map(|(pid, _)| pid).collect()
}

/// Starts what `mode` needs, in order. Runs on its own thread.
fn launch(p: &Paths, mode: Mode, link: &str, shared: &Arc<Mutex<Shared>>) -> Result<(), String> {
    if let Some(problem) = p.problems().first() {
        return Err(problem.clone());
    }
    if mode != Mode::Join && !p.can_host() {
        return Err("This copy has no WoW server: it can only join friends.".into());
    }
    if running().iter().any(|x| x == "benilla") {
        return Err("WoW is already running. Press Stop first.".into());
    }
    let say = |text: &str| step(shared, text);
    if p.packaged {
        let server = p.server();
        if mode == Mode::Join {
            // Our own server holds the WoW ports a friend's server is reached through.
            if setup::listening(setup::REALM_PORT) || setup::listening(setup::WORLD_PORT) {
                say("Stopping your own server first...");
                server.stop();
            }
        } else {
            if !server.has_maps() {
                let progress = |f: f32| shared.lock().unwrap().progress = Some(f);
                let result = server.extract(&p.wow_dir(), &say, &progress);
                shared.lock().unwrap().progress = None;
                result?;
            }
            server.start(&say)?;
            // Creature pathfinding: built in the background the first time, used from the next start.
            server.build_mmaps_in_background(&running());
        }
        say("Setting up Minecraft in Prism...");
        p.install_instance()?;
        minecraft_settings(p, mode, link);
        let before = java_pids();
        start_prism(p)?;
        say("Starting Minecraft in Prism. The first time, sign in there; it then downloads Minecraft (a few minutes).");
        let asked = Instant::now();
        while !java_pids().iter().any(|pid| !before.contains(pid)) {
            if asked.elapsed() > Duration::from_secs(20 * 60) {
                return Err("Minecraft didn't start. Check Prism Launcher (signed in?), then try again.".into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    } else if mode != Mode::Join {
        start_dev_server(p, shared)?;
    }

    say("Starting WoW...");
    let log = |name: &str| std::fs::File::create(p.benilla().join(name)).map(Stdio::from).unwrap_or_else(|_| Stdio::null());
    let benilla = p.benilla_exe();
    // Paths as WoW takes them (Windows ones under Wine).
    let path = |f: &Path| benilla.path_arg(f);
    let mut wow = benilla.command();
    wow.current_dir(p.benilla())
        .env("WOW_DATA", path(&p.data_dir()))
        .env("WOWCRAFT", "1")
        .env("WOW_NOVSYNC", "1")
        // Our Minecraft skin (saved by the mod), for the character screens' Minecraft body.
        .env("WOWCRAFT_SKIN_FILE", path(&p.game_dir().join("wowcraft-skin.png")))
        // And our Minecraft name, which character creation offers.
        .env("WOWCRAFT_NAME_FILE", path(&p.game_dir().join("wowcraft-name.txt")))
        .stdout(log("run_out.txt"))
        .stderr(log("run_err.txt"));
    // A guest's account comes from the friend's server; the packaged install's own from its
    // server, made the same way when Minecraft enters our world.
    if mode == Mode::Join || p.packaged {
        let _ = std::fs::remove_file(p.account_file());
        wow.env("WOW_HOST", "127.0.0.1")
            .env("WOWCRAFT_ACCOUNT_FILE", path(&p.account_file()))
            .env("WOW_ALLOW_ACCOUNT", "1")
            .env_remove("WOW_USER")
            .env_remove("WOW_PASS");
    }
    wow.spawn().map_err(|e| format!("couldn't start WoW: {e}"))?;
    std::thread::sleep(Duration::from_secs(5));

    if !p.packaged {
        say("Starting Minecraft...");
        minecraft_settings(p, mode, link);
        let log = p.root.join("wowcraft/runclient.log").display().to_string();
        let mut mc = if cfg!(windows) {
            let mut mc = platform::command("cmd");
            mc.args(["/c", &format!("gradlew.bat runClient --no-daemon > {log} 2>&1")]);
            mc
        } else {
            let mut mc = Command::new("sh");
            mc.args(["-c", &format!("./gradlew runClient --no-daemon > '{log}' 2>&1")]);
            mc
        };
        mc.current_dir(p.fabric()).env("SKYCRAFT_USERNAME", &p.name).env("WOWCRAFT_MARIADB", p.server().mariadb_client());
        // The checkout's own Java, if it has one.
        if p.root.join("jdk25").is_dir() {
            mc.env("JAVA_HOME", p.root.join("jdk25"));
        }
        if !p.skin.is_empty() {
            mc.env("WOWCRAFT_SKIN", &p.skin);
        }
        if mode == Mode::Host {
            // The dev client plays offline, so its world can't verify anyone: friends are let in
            // (and given WoW accounts) by name. Share the link with friends only.
            mc.env("SKYCRAFT_LAN_PORT", "25566").env("SKYCRAFT_LAN_OFFLINE", "1").env("WOWCRAFT_ALLOW_OFFLINE_GUESTS", "1");
        } else {
            mc.env_remove("SKYCRAFT_LAN_PORT");
        }
        let child = mc.spawn().map_err(|e| format!("couldn't start Minecraft: {e}"))?;
        shared.lock().unwrap().minecraft = Some(child);
    }
    say(match (mode, p.packaged) {
        (Mode::Join, _) => "Joining your friend... WoW logs in by itself once Minecraft is in.",
        (_, true) => "Starting... WoW logs in by itself once Minecraft is in. Make a character the first time.",
        (Mode::Solo, false) => "Log in and Enter World in WoW.",
        (Mode::Host, false) => "Enter World in WoW; the join link appears here.",
    });
    Ok(())
}

/// A small status dot and its name, for the footer.
fn light(ui: &mut egui::Ui, on: bool, label: &str) {
    let colour = if on { Color32::from_rgb(90, 210, 110) } else { Color32::from_gray(70) };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, colour);
    ui.label(RichText::new(label).small().color(if on { Color32::from_gray(200) } else { MUTED }));
    ui.add_space(6.0);
}

/// A filled call-to-action button of the given size.
fn primary(ui: &mut egui::Ui, size: [f32; 2], text: &str, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(text).size(20.0).strong().color(if enabled { INK } else { MUTED }))
        .fill(if enabled { GOLD } else { Color32::from_rgb(60, 56, 46) });
    ui.add_enabled(enabled, egui::Button::min_size(button, size.into())).clicked()
}

/// A section label with rules either side ("or join a friend").
fn rule_label(ui: &mut egui::Ui, width: f32, text: &str) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(13.0), MUTED);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::hover());
    let mid = rect.center();
    let half = galley.size().x * 0.5 + 10.0;
    let stroke = egui::Stroke::new(1.0_f32, Color32::from_gray(60));
    ui.painter().line_segment([egui::pos2(rect.left(), mid.y), egui::pos2(mid.x - half, mid.y)], stroke);
    ui.painter().line_segment([egui::pos2(mid.x + half, mid.y), egui::pos2(rect.right(), mid.y)], stroke);
    ui.painter().galley(egui::pos2(mid.x - galley.size().x * 0.5, rect.top()), galley, MUTED);
}

impl eframe::App for App {
    /// Closing the launcher ends the background mmaps build (the game it ran for may stay).
    /// Closing the launcher closes the game it started: WoW, Minecraft (saved first) and, for the
    /// packaged install, its server and the background mmaps build.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.paths.packaged {
            self.poll();
            if self.up("benilla") || self.minecraft_up() || self.server_up || self.up("mariadbd") {
                stop_all(&self.paths, &self.shared, true);
            } else {
                self.paths.server().stop_mmaps();
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.polled.elapsed() > Duration::from_secs(2) {
            self.poll();
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let (busy, status, error, progress, available) = {
            let s = self.shared.lock().unwrap();
            (s.busy, s.step.clone(), s.error.clone(), s.progress, s.update.clone())
        };
        // A copy without a server only joins: just the join box.
        let friend = !self.paths.can_host();
        let wow_up = self.up("benilla");
        let minecraft_up = self.minecraft_up();
        let server_up = self.server_up;
        let playing = wow_up || minecraft_up;
        // The game was closed from inside: back to the start screen.
        if self.mode.is_some() && !busy && !playing {
            self.mode = None;
            if self.paths.packaged {
                // WoW was exited from inside: its server (ours alone) goes with it, as on Stop.
                let paths = self.paths.clone();
                let shared = self.shared.clone();
                shared.lock().unwrap().step = "The game closed; stopping the server...".into();
                std::thread::spawn(move || {
                    stop_all(&paths, &shared, true);
                    shared.lock().unwrap().step = "Stopped.".into();
                });
            }
        }
        // The window fits its page: a friend's start screen is only the join box.
        let height = match (friend, self.show_settings) {
            (true, false) if self.mode.is_none() && !busy => 330.0,
            (false, true) => 430.0,
            _ => 400.0,
        };
        // The WowCraft folder's own row in Settings.
        let height = if self.show_settings && !self.paths.dev() { height + 90.0 } else { height };
        // The start screen's warning box (no WoW folder yet, say) sits above the buttons.
        let warned = !self.show_settings && self.mode.is_none() && !busy && !self.paths.problems().is_empty();
        let height = if warned { height + 30.0 + 22.0 * self.paths.problems().len() as f32 + 40.0 } else { height };
        // The update offer above the buttons.
        let offered = !self.show_settings && self.mode.is_none() && !busy && available.is_some() && self.paths.packaged;
        let height = if offered { height + 52.0 } else { height };
        if self.height != height {
            self.height = height;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(440.0, height)));
        }
        let link = std::fs::read_to_string(self.paths.link_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::new().fill(Color32::from_rgb(30, 26, 18)).inner_margin(egui::Margin::symmetric(0, 14)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("WowCraft").size(32.0).strong().color(GOLD));
                    ui.label(RichText::new("Minecraft in World of Warcraft").color(Color32::from_gray(185)));
                });
            });

        egui::TopBottomPanel::bottom("footer")
            .frame(egui::Frame::new().fill(Color32::from_rgb(17, 17, 20)).inner_margin(egui::Margin::symmetric(14, 8)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if !friend {
                        light(ui, server_up, "Server");
                    }
                    light(ui, wow_up, "WoW");
                    light(ui, minecraft_up, "Minecraft");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.show_settings { "Back" } else { "Settings" };
                        if ui.add(egui::Button::new(RichText::new(label).size(14.0)).frame(false)).clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        ui.label(RichText::new(format!("v{}", update::VERSION)).small().color(Color32::from_gray(90)));
                    });
                });
            });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::symmetric(0, 18))).show(ctx, |ui| {
            const W: f32 = 360.0;
            ui.vertical_centered(|ui| {
                ui.set_max_width(W);
                if self.show_settings {
                    self.settings(ui, W);
                    return;
                }

                let problems = self.paths.problems();
                if !problems.is_empty() && self.mode.is_none() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(52, 30, 28))
                        .corner_radius(6)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(W - 20.0);
                            for p in &problems {
                                ui.label(RichText::new(p).color(ERROR));
                            }
                            if ui.link("Open settings").clicked() {
                                self.show_settings = true;
                            }
                        });
                    ui.add_space(4.0);
                }

                if self.mode.is_some() || busy {
                    // Playing (or starting, or stopping): what's happening, the link to share, Stop.
                    ui.add_space(6.0);
                    // The spinner over the text, which wraps inside the column.
                    if busy {
                        ui.spinner();
                    }
                    ui.add(egui::Label::new(RichText::new(if status.is_empty() { "Running." } else { status.as_str() }).size(16.0)).wrap());
                    if let Some(p) = progress {
                        ui.add(egui::ProgressBar::new(p).desired_width(W).show_percentage().fill(Color32::from_rgb(170, 130, 45)));
                    }
                    if let Some(e) = &error {
                        ui.label(RichText::new(e).color(ERROR));
                    }
                    if self.mode == Some(Mode::Host) {
                        ui.add_space(8.0);
                        egui::Frame::new().fill(Color32::from_rgb(34, 32, 26)).corner_radius(6).inner_margin(12).show(ui, |ui| {
                            ui.set_width(W - 24.0);
                            ui.vertical_centered(|ui| match &link {
                                Some(l) => {
                                    ui.label(RichText::new("Friends join with").color(MUTED));
                                    // Hidden until asked: anyone who sees it can join (and gets a WoW account).
                                    let shown = if self.show_link { l.clone() } else { "*".repeat(l.len().min(24)) };
                                    ui.label(RichText::new(shown).size(18.0).strong().color(GOLD));
                                    ui.horizontal(|ui| {
                                        let copied = self.copied.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
                                        let copy = ui.button(if copied { "Copied!" } else { "Copy link" });
                                        let show = ui.button(if self.show_link { "Hide" } else { "Show" });
                                        if copy.clicked() {
                                            ui.ctx().copy_text(l.clone());
                                            self.copied = Some(Instant::now());
                                        }
                                        if show.clicked() {
                                            self.show_link = !self.show_link;
                                        }
                                    });
                                }
                                None => {
                                    ui.label(RichText::new("Your join link shows up here once you're in the world.").color(MUTED));
                                }
                            });
                        });
                    }
                    ui.add_space(14.0);
                    let stop = egui::Button::new(RichText::new("Stop").size(17.0)).fill(Color32::from_rgb(110, 40, 36));
                    if ui.add_enabled(!busy || playing, egui::Button::min_size(stop, egui::vec2(W, 42.0))).clicked() {
                        // The packaged install's server is ours alone: Stop takes it down too.
                        let server_too = self.stop_server || self.paths.packaged;
                        self.stop(server_too);
                    }
                    if !friend && !self.paths.packaged {
                        ui.checkbox(&mut self.stop_server, RichText::new("Also stop the WoW server").small().color(MUTED));
                    }
                    return;
                }

                // The start screen, under a newer version's offer.
                if let Some(release) = available.filter(|_| self.paths.packaged) {
                    egui::Frame::new().fill(Color32::from_rgb(34, 44, 30)).corner_radius(6).inner_margin(10).show(ui, |ui| {
                        ui.set_width(W - 20.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("WowCraft {} is out", release.version)).color(Color32::from_rgb(170, 230, 140)));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("Update").on_hover_text("Saves and settings are kept").clicked() {
                                    self.start_update(release.clone());
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
                let ready = problems.is_empty();
                if !friend {
                    if primary(ui, [W, 56.0], "PLAY", ready) {
                        self.start(Mode::Solo);
                    }
                    let host = egui::Button::new(RichText::new("Host co-op").size(16.0)).min_size(egui::vec2(W, 38.0));
                    if ui.add_enabled(ready, host).on_hover_text("Play, and let friends join you with a link").clicked() {
                        self.start(Mode::Host);
                    }
                    ui.add_space(10.0);
                    rule_label(ui, W, "or join a friend");
                    ui.add_space(4.0);
                } else {
                    ui.label(RichText::new("Join a friend").size(20.0).strong());
                    ui.label(RichText::new("Paste the link they sent you.").color(MUTED));
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    let join_w = 96.0;
                    let field_w = W - join_w - ui.spacing().item_spacing.x;
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.join_link)
                            .hint_text("something.e4mc.link")
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(10, 0))
                            // The width is the text's; the margins come on top.
                            .min_size(egui::vec2(0.0, 42.0))
                            .desired_width(field_w - 20.0),
                    );
                    let ok = ready && !self.join_link.trim().is_empty();
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let join = if friend {
                        primary(ui, [join_w, 42.0], "Join", ok)
                    } else {
                        ui.add_enabled(ok, egui::Button::new("Join").min_size(egui::vec2(join_w, 42.0))).clicked()
                    };
                    if join || (enter && ok) {
                        self.start(Mode::Join);
                    }
                });
                if let Some(e) = &error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(ERROR));
                } else if !status.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&status).color(MUTED));
                }
            });
        });
    }
}

impl App {
    /// The settings page: where WoW is, and Minecraft's side (Prism, or the dev client's name and skin).
    fn settings(&mut self, ui: &mut egui::Ui, w: f32) {
        ui.label(RichText::new("Settings").size(20.0).strong());
        ui.add_space(6.0);
        let mut changed = false;
        let field = |ui: &mut egui::Ui, value: &mut String, hint: &str, width: f32| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .vertical_align(egui::Align::Center)
                    .min_size(egui::vec2(0.0, 34.0))
                    .desired_width(width - 8.0),
            )
                .changed()
        };
        let browse_w = 100.0;
        let gap = ui.spacing().item_spacing.x;

        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(w);
            if !self.paths.dev() {
                // The launcher needn't sit in the WowCraft folder (an AppImage kept elsewhere, say).
                ui.label("WowCraft folder");
                let mut root = self.paths.root.display().to_string();
                let mut picked = false;
                ui.horizontal(|ui| {
                    picked |= field(ui, &mut root, "the unzipped WowCraft folder", w - browse_w - gap);
                    if ui.add(egui::Button::new("Browse...").min_size(egui::vec2(browse_w, 34.0))).clicked() {
                        if let Some(dir) = rfd::FileDialog::new().set_title("Your WowCraft folder").pick_folder() {
                            root = dir.display().to_string();
                            picked = true;
                        }
                    }
                });
                let root = PathBuf::from(root.trim().trim_matches('"'));
                if picked && benilla_in(&root).exists() {
                    let file = chosen_root_file();
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(&file, format!("root={}\n", root.display()));
                    // Its own settings now; a WoW folder chosen before carries over if it has none.
                    let wow_data = self.paths.wow_data.clone();
                    self.paths = Paths::detect();
                    if !read_cfg(&self.paths.cfg_file()).contains_key("wow_data") {
                        self.paths.wow_data = wow_data;
                        self.paths.save();
                    }
                } else if !self.paths.packaged {
                    ui.label(RichText::new("Not found yet: pick the folder with wow, server and minecraft in it.").small().color(ERROR));
                }
                ui.add_space(6.0);
            }
            ui.label("WoW 1.12 folder");
            let mut data = self.paths.wow_data.display().to_string();
            ui.horizontal(|ui| {
                changed |= field(ui, &mut data, "the folder with WoW.exe and Data", w - browse_w - gap);
                if ui.add(egui::Button::new("Browse...").min_size(egui::vec2(browse_w, 34.0))).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Your WoW 1.12 folder").pick_folder() {
                        data = dir.display().to_string();
                        changed = true;
                    }
                }
            });
            ui.add_space(6.0);
            if self.paths.packaged && cfg!(not(windows)) {
                ui.label(RichText::new("Minecraft runs through the bundled Prism Launcher, under Wine with WoW.").small().color(MUTED));
            } else if self.paths.packaged {
                ui.label("Prism Launcher");
                let mut prism = self.paths.prism.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                ui.horizontal(|ui| {
                    changed |= field(ui, &mut prism, "the bundled one", w - browse_w - gap);
                    if ui.add(egui::Button::new("Browse...").min_size(egui::vec2(browse_w, 34.0))).clicked() {
                        if let Some(exe) = rfd::FileDialog::new().set_title("prismlauncher.exe").add_filter("Prism Launcher", &["exe"]).pick_file() {
                            prism = exe.display().to_string();
                            changed = true;
                        }
                    }
                });
                ui.label(RichText::new("Minecraft runs with the account you're signed into in Prism.").small().color(MUTED));
                let prism = prism.trim().trim_matches('"').to_string();
                if changed {
                    self.paths.prism = if prism.is_empty() { None } else { Some(PathBuf::from(prism)) };
                }
            } else if self.paths.dev() {
                ui.label("Minecraft name");
                changed |= field(ui, &mut self.paths.name, "your name in Minecraft", w);
                ui.add_space(6.0);
                ui.label("Skin (a Minecraft account's name)");
                changed |= field(ui, &mut self.paths.skin, "optional", w);
            }
            if changed {
                self.paths.wow_data = PathBuf::from(data.trim().trim_matches('"'));
                self.paths.save();
            }
            ui.add_space(10.0);
            ui.label(RichText::new(format!("Installed in {}", self.paths.root.display())).small().color(MUTED));
        });
    }
}

/// `WowCraft.exe --server`: sets up (the first time) and starts the packaged WoW server without
/// a window, logging to wowcraft-server.log beside the launcher; the server keeps running after.
fn server_only() {
    let paths = Paths::detect();
    let log_path = paths.root.join("wowcraft-server.log");
    let log = Mutex::new(std::fs::File::create(&log_path).ok());
    let say = |text: &str| {
        if let Some(f) = log.lock().unwrap().as_mut() {
            use std::io::Write;
            let _ = writeln!(f, "{text}");
        }
    };
    if !paths.packaged {
        say("--server is for the packaged install.");
        return;
    }
    let server = paths.server();
    let started = Instant::now();
    let result = (|| {
        if !server.has_maps() {
            if let Some(problem) = paths.wow_problem() {
                return Err(problem);
            }
            server.extract(&paths.wow_dir(), &say, &|_| {})?;
            say(&format!("maps extracted in {} s", started.elapsed().as_secs()));
        }
        server.start(&say)?;
        server.build_mmaps_in_background(&running());
        Ok::<(), String>(())
    })();
    match result {
        Ok(()) => say(&format!("server up after {} s", started.elapsed().as_secs())),
        Err(e) => say(&format!("error: {e}")),
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--server") {
        server_only();
        return Ok(());
    }
    let icon = egui::IconData { rgba: include_bytes!("../assets/icon.rgba").to_vec(), width: 64, height: 64 };
    let height = if Paths::detect().can_host() { 400.0 } else { 330.0 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, height])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(icon)
            .with_title("WowCraft"),
        ..Default::default()
    };
    eframe::run_native("WowCraft", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A friend's copy installs its instance into a portable Prism, replaces an older WowCraft jar
    /// and keeps the player's own mods.
    /// Minecraft counts as running while its log is held open, not while some other Java runs.
    #[test]
    fn minecraft_is_up_while_its_log_is_open() {
        let dir = std::env::temp_dir().join(format!("wowcraft-log-test-{}", std::process::id()));
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        assert!(!minecraft_running(&dir), "no log yet");
        let held = std::fs::File::create(logs.join("latest.log")).unwrap();
        assert!(minecraft_running(&dir), "log held open");
        drop(held);
        assert!(!minecraft_running(&dir), "log closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_instance_lands_in_prism_and_refreshes_our_mods() {
        let tmp = std::env::temp_dir().join(format!("wowcraft-launcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("WowCraft");
        let src = root.join("minecraft").join(INSTANCE);
        std::fs::create_dir_all(src.join("minecraft/mods")).unwrap();
        std::fs::write(src.join("instance.cfg"), "[General]\n").unwrap();
        std::fs::write(src.join("mmc-pack.json"), "{}").unwrap();
        std::fs::write(src.join("minecraft/mods/wowcraft-0.2.0.jar"), "new").unwrap();
        let prism_dir = tmp.join("Prism");
        std::fs::create_dir_all(&prism_dir).unwrap();
        std::fs::write(prism_dir.join("portable.txt"), "").unwrap();
        std::fs::write(prism_dir.join("prismlauncher.exe"), "").unwrap();
        let mods = prism_dir.join("instances").join(INSTANCE).join("minecraft/mods");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::write(mods.join("wowcraft-0.1.2.jar"), "old").unwrap();
        std::fs::write(mods.join("sodium-1.0.jar"), "theirs").unwrap();

        let paths = Paths {
            root: root.clone(),
            packaged: true,
            wow_data: tmp.join("WoW"),
            skin: String::new(),
            name: String::new(),
            prism: Some(prism_dir.join("prismlauncher.exe")),
        };
        assert_eq!(paths.game_dir(), prism_dir.join("instances").join(INSTANCE).join("minecraft"));
        paths.install_instance().unwrap();
        assert!(prism_dir.join("instances").join(INSTANCE).join("mmc-pack.json").exists());
        assert_eq!(std::fs::read_to_string(mods.join("wowcraft-0.2.0.jar")).unwrap(), "new");
        assert!(!mods.join("wowcraft-0.1.2.jar").exists());
        assert!(mods.join("sodium-1.0.jar").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The WoW folder or its Data folder both work.
    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wowcraft-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn paths_at(root: &Path, wow: &Path) -> Paths {
        Paths { root: root.to_path_buf(), packaged: true, wow_data: wow.to_path_buf(), skin: String::new(), name: String::new(), prism: None }
    }

    /// Only the 1.12 client passes; TBC/Wrath, today's WoW and a wrong folder each say what they are.
    #[test]
    fn the_wow_folder_must_be_the_1_12_client() {
        let root = tmp("wowcheck");
        let check = |files: &[&str]| {
            let wow = root.join(format!("wow{}", files.len() * 7 + files.first().map_or(0, |f| f.len())));
            std::fs::create_dir_all(wow.join("Data")).unwrap();
            for f in files {
                let path = wow.join(f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "").unwrap();
            }
            paths_at(&root, &wow).wow_problem()
        };
        assert_eq!(check(&["Data/dbc.MPQ", "Data/patch.MPQ"]), None, "1.12 passes");
        assert!(check(&["Data/patch.MPQ", "Data/common.MPQ", "Data/expansion.MPQ"]).unwrap().contains("later WoW"), "TBC");
        assert!(check(&["Data/patch.MPQ", "Data/lichking.MPQ"]).unwrap().contains("later WoW"), "Wrath");
        assert!(check(&[".build.info", "Data/data/0000000001.idx"]).unwrap().contains("today's WoW"), "Classic/Retail");
        assert!(check(&["readme.txt"]).unwrap().contains("not found"), "wrong folder");
        assert!(paths_at(&root, Path::new("")).wow_problem().unwrap().contains("Choose"), "none chosen yet");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder chosen for Prism means its program (Windows "access denied" for running a folder).
    #[cfg(windows)]
    #[test]
    fn a_prism_folder_means_its_program() {
        let dir = tmp("prismdir");
        assert_eq!(prism_exe(dir.clone()), dir.join("prismlauncher.exe"));
        let exe = dir.join("prismlauncher.exe");
        std::fs::write(&exe, "").unwrap();
        assert_eq!(prism_exe(exe.clone()), exe);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in Prism: Windows' where.exe, which starts and quits at once.
    #[cfg(windows)]
    fn fake_prism(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("portable.txt"), "").unwrap();
        let exe = dir.join("prismlauncher.exe");
        std::fs::copy(Path::new(&std::env::var("SystemRoot").unwrap()).join("System32").join("where.exe"), &exe).unwrap();
        exe
    }

    /// Windows refuses to run it: the same "Access is denied. (os error 5)" as the report.
    #[cfg(windows)]
    fn deny_running(exe: &Path) {
        let ok = Command::new("icacls").arg(exe).args(["/deny", "*S-1-1-0:(RX)"]).stdout(Stdio::null()).status().unwrap().success();
        assert!(ok, "icacls deny");
    }

    #[cfg(windows)]
    fn allow_running(exe: &Path) {
        let _ = Command::new("icacls").arg(exe).args(["/remove:d", "*S-1-1-0"]).stdout(Stdio::null()).status();
    }

    /// An installed Prism that Windows won't start: the bundled one starts instead, with the
    /// instance copied into it; with no bundled one, the message says what to do.
    #[cfg(windows)]
    #[test]
    fn a_blocked_prism_falls_back_to_the_bundled_one() {
        let root = tmp("prismstart");
        let src = root.join("minecraft").join(INSTANCE);
        std::fs::create_dir_all(src.join("minecraft").join("mods")).unwrap();
        std::fs::write(src.join("instance.cfg"), "name=WowCraft").unwrap();
        std::fs::write(src.join("mmc-pack.json"), "{}").unwrap();
        std::fs::write(src.join("minecraft").join("mods").join("wowcraft-1.jar"), "jar").unwrap();
        let bundled = fake_prism(&root.join("prism"));
        let other = fake_prism(&root.join("other-prism"));
        let mut p = paths_at(&root, &root);
        p.prism = Some(other.clone());
        // The report first: starting the blocked one really is "access denied".
        deny_running(&other);
        let raw = Command::new(&other).spawn().map(|mut c| { let _ = c.wait(); }).unwrap_err();
        assert_eq!(raw.raw_os_error(), Some(5), "reproduced: {raw}");
        start_prism(&p).expect("the bundled Prism starts");
        assert!(root.join("prism").join("instances").join(INSTANCE).join("instance.cfg").is_file(), "instance copied to the bundled Prism");
        // No bundled one to fall back to: a message that says what to do.
        // The stand-in may still be running for a moment.
        let gone = (0..50).any(|_| std::fs::remove_file(&bundled).is_ok() || { std::thread::sleep(Duration::from_millis(100)); false });
        assert!(gone, "bundled stand-in removed");
        let err = start_prism(&p).unwrap_err();
        assert!(err.contains("Windows refused") && err.contains("antivirus"), "{err}");
        allow_running(&other);
        std::thread::sleep(Duration::from_millis(300));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_wow_folder_finds_its_data() {
        let tmp = std::env::temp_dir().join(format!("wowcraft-launcher-data-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("Data")).unwrap();
        let mut paths = Paths { root: tmp.clone(), packaged: true, wow_data: tmp.clone(), skin: String::new(), name: String::new(), prism: None };
        assert_eq!(paths.data_dir(), tmp.join("Data"));
        paths.wow_data = tmp.join("Data");
        assert_eq!(paths.data_dir(), tmp.join("Data"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
