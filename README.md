# WowCraft

Minecraft inside World of Warcraft 1.12. Runs on your PC with its own WoW server. Co-op with friends.

<p align="center">
  <a href="https://youtu.be/sX3lsLiPS2s">
    <img src="https://img.youtube.com/vi/sX3lsLiPS2s/maxresdefault.jpg" alt="Watch WowCraft on YouTube" width="640">
  </a>
  <br>
  <sub>▶ Watch on YouTube</sub>
</p>

<p align="center">
  <img src="docs/launcher.png" alt="WowCraft launcher" width="431">
</p>

> **Disclaimer.** WowCraft does **not** include or distribute World of Warcraft, its client, or
> any Blizzard game files, and it does not include Minecraft. You need your own WoW 1.12.1
> client and your own Minecraft Java Edition account. The server's map data is extracted on
> your PC from your client the first time you play.
>
> Fan project. Not affiliated with Blizzard Entertainment, Mojang Studios or Microsoft.
> World of Warcraft is a trademark of Blizzard Entertainment; Minecraft is a trademark of Mojang Studios.

> **Early test build.** Expect bugs and things that don't work yet. Back up your WowCraft folder if you care about your saves.

## Requirements

| | |
|---|---|
| WoW client | 1.12.1 client folder (with `WoW.exe` and `Data`) |
| Minecraft | Java Edition, your own Microsoft account |
| OS | Windows 10 or 11, 64-bit, or Linux x86_64 (see [Linux](#linux)) |
| Disk | ~4 GB free |

## Install

1. Download `WowCraft.zip` from [Releases](../../releases/latest).
2. Unzip anywhere (not inside `Program Files`).
3. Run `WowCraft.exe`.
4. Settings → Browse → pick your WoW 1.12 folder.
5. Press **PLAY**.

## Linux

The Linux launcher is an AppImage. It runs the same WowCraft folder: Linux builds of its programs
where the folder has them, the Windows ones through Wine otherwise. Minecraft runs through the
bundled Prism Launcher under Wine too, in the same Wine prefix as WoW (the WowCraft mod talks to
WoW through Windows shared memory). Sign in to Prism there the first time.

| | |
|---|---|
| Wine | Your distribution's `wine` package (WoW, Minecraft and the server run through it) |
| MariaDB | Optional: a system `mariadbd` is used before the bundled Windows one |
| Linux | x86_64, glibc 2.30 or newer (any current distribution) |

1. Download `WowCraft.zip` and `WowCraft-x86_64.AppImage` from [Releases](../../releases/latest).
   The zip is the same one Windows uses: the game, the server and Prism are in it.
2. Unzip `WowCraft.zip` anywhere in your home folder.
3. Make `WowCraft-x86_64.AppImage` executable (`chmod +x`) and run it. Kept in the `WowCraft`
   folder, it finds it by itself; kept elsewhere, pick the folder in Settings → WowCraft folder.
4. Settings → Browse → pick your WoW 1.12 folder, and press **PLAY**.

On the first PLAY, Prism opens under Wine: sign in with your Microsoft account there (it's a
separate Prism from any you have installed). It then downloads Minecraft and Java (once).

WowCraft's Windows programs run in a Wine prefix of their own (`~/.local/share/WowCraft/wine`,
or `$WINEPREFIX` if set); `$WINE` picks a Wine other than the one on your `PATH`.

## First start

1. Maps are extracted from your WoW client (~10–15 min, once).
2. Prism Launcher opens: sign in with your Microsoft account. It downloads Minecraft and Java (once).
3. WoW opens, waits for Minecraft, then logs in by itself. Make a character, Enter World.

Pathfinding builds in the background for about an hour; you can play meanwhile.

## Keybinds

**Minecraft**

| Key | Action |
|---|---|
| `Space` | Jump / get off a boat, zeppelin or tram |
| `E` | Inventory |
| `T` | Minecraft chat |
| `Esc` | Minecraft menu |

**WoW**

| Key | Action |
|---|---|
| `C` | WoW cursor on/off (NPCs, loot, quests) |
| `Enter` | WoW chat |
| `M` | Map |
| `L` | Quest log |
| `B` | Bags |
| `O` | WoW menu |

## Commands

Type these in WoW chat (`Enter`). Use WoW character names.

| Command | What it does |
|---|---|
| `.goname Name` | Teleport to a player (their WoW character name) |
| `.namego Name` | Teleport a player to you (their WoW character name) |
| `.levelup 10` | Gain 10 levels. To level a friend, first `/target Name` (their WoW character name) |
| `.tele Orgrimmar` | Go to a place |
| `.lookup tele storm` | Find place names for `.tele` |
| `.go xyz X Y Z map` | Go to exact coordinates |
| `.gps` | Show your coordinates |
| `.gm on` / `.gm off` | Creatures ignore you |

In Minecraft chat (`T`): `/gamemode creative`, `/gamemode survival`.

## Co-op

| | |
|---|---|
| Host | Press **Host co-op**, then **Copy** the link that appears and send it. |
| Join | Paste a friend's link, press **Join**. Your WoW account is made for you. |

Everyone needs their own install, WoW client and Minecraft account.

## Saving

**Stop** (or closing the launcher) closes WoW, Minecraft and the server, saving everything.
Saves live in the WowCraft folder (`mariadb\data` and Prism's `WowCraft` instance).

## Updating

The launcher checks for new versions on start. When one is out, press **Update**. Saves and
settings are kept.

Manually: unzip a newer `WowCraft.zip` over your WowCraft folder. Same result. On Linux, also
replace your `WowCraft-x86_64.AppImage` with the release's.

## Troubleshooting

| Problem | Fix |
|---|---|
| WoW 1.12 not found | Pick the folder that has `WoW.exe` and `Data` in it. |
| First setup failed | Check `server\data\*.log`, press PLAY again. |
| Minecraft doesn't start | Sign in to Prism (Accounts, top right), press PLAY again. |
| Stuck on "Please wait" | Minecraft is still loading. When joining: check the link. |
| Port in use | Close any other WoW server or database. |
| Linux: "Wine is needed" | Install your distribution's `wine` package. |
| Linux: first setup failed | Check `server/data/*.log` (they include Wine's errors), press PLAY again. |

Logs: `wow\run_err.txt` (WoW), `server\logs` (server), Prism's instance log (Minecraft:
`prism/instances/WowCraft/minecraft/logs/latest.log`).

## Known issues

- Boats, trams and zeppelins may work weird.

## What's included

Licences are in the download's `licenses` folder.

- **Benilla** — WoW client (`wow\benilla.exe`), MIT or Apache-2.0, with WowCraft's changes
- **[vmangos](https://github.com/vmangos/core)** — WoW server (`server\`), GPL-2.0
- **[MariaDB](https://github.com/MariaDB/server)** — database (`mariadb\`), GPL-2.0
- **[Prism Launcher](https://github.com/PrismLauncher/PrismLauncher)** — Minecraft launcher (`prism\`), GPL-3.0
- **[Fabric API](https://github.com/FabricMC/fabric)** — Apache-2.0
- **[e4mc](https://github.com/vgskye/e4mc-minecraft-architectury)** — co-op links, MIT
- **[Lithium](https://github.com/CaffeineMC/lithium)**, **[FerriteCore](https://github.com/malte0811/FerriteCore)** — performance, LGPL-3.0 / MIT
- **WowCraft mod** and **launcher** — MIT
- Microsoft Visual C++ runtime DLLs

No Blizzard or Mojang files are included.

## Building the launcher

```
cd launcher
cargo build --release
```

The Linux AppImage (in `launcher/target/appimage/`):

```
cd launcher
linux/build-appimage.sh            # in a Debian 11 container (Docker or Podman): runs on glibc 2.30+
linux/build-appimage.sh --native   # with this machine's Rust
```

## Making a release

1. Bump `version` in `launcher/Cargo.toml` (the launcher compares it with the release tag to
   offer updates).
2. Commit, then tag and push: `git tag v0.1.2 && git push origin main v0.1.2`.
3. GitHub Actions ([`.github/workflows/linux-appimage.yml`](.github/workflows/linux-appimage.yml))
   builds `WowCraft-x86_64.AppImage` and attaches it to the `v0.1.2` release, creating the
   release if there isn't one yet.
4. Upload `WowCraft.zip` (the Windows build of the whole package) to the same release.

Both files must be on the release: Windows players need the zip; Linux players need the zip and
the AppImage. The workflow can also be started by hand (Actions → Linux AppImage → Run
workflow) to build an AppImage without releasing it.

## License

Launcher source: [MIT](LICENSE).
