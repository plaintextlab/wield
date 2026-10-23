# Wield user manual

Wield is a Windows toolkit for Windows 10 and Windows 11 (Home, Pro, Enterprise, Education, LTSC and IoT). Its tools
let you install and update apps, control Windows Update, fix and update drivers, manage keyboard shortcuts, control what starts with Windows, protect your privacy, remove bloat, watch and control the network, scan
for malware, manage processes, back up and restore the whole PC, and edit the right-click menus.

This manual explains every screen and every button. Each tool has its own page.

![Home screen](images/home.png)

## Contents

- [Getting started](#getting-started)
  - [Installing](#installing)
  - [The window](#the-window)
  - [PC health](#pc-health)
  - [Administrator rights](#administrator-rights)
  - [How Wield keeps you safe](#how-wield-keeps-you-safe)
- Tools
  - [Startup Manager](startup.md): apps, services and tasks that start with Windows
  - [Privacy & Telemetry](privacy.md): turn off data collection, ads and tracking
  - [App Debloater](debloat.md): remove preinstalled Store apps
  - [Process Explorer](processes.md): every running process, with full control
  - [Malware Scanner](malware.md): Defender scans, signatures, online reputation, exclusions and quarantine
  - [Network Toolkit](network.md): per-app traffic, blocking, ports, firewall, LAN scanner, DNS and repairs
  - [App Installer](apps.md): install 90 free essential apps at once, silently
  - [Driver Manager](drivers.md): missing and outdated drivers from Windows Update, NVIDIA and your PC's maker
  - [Keyboard Shortcuts](shortcuts.md): every shortcut in one place; turn them off or give programs their own keys
  - [Update Control](updates.md): pause or run Windows Update, and update your apps
  - [Backup & Restore](backup.md): system images, System Restore, folder backups and a reinstall kit
  - [Context Menu](context-menu.md): every right-click entry; hide, delete, back up, add your own
- [Settings and updates](settings.md)
- [Where Wield keeps its data](#where-wield-keeps-its-data)
- [Command-line options](#command-line-options)
- [Troubleshooting](#troubleshooting)

## Getting started

### Installing

Download Wield from the [Releases page](https://github.com/plaintextlab/wield/releases). There are two kinds:

| File | What it is |
|---|---|
| `wield-vX.Y.Z-setup.exe` | **Installer.** Installs for your account without admin rights, or for all users. Adds a Start menu entry, an optional desktop shortcut and an uninstaller in Settings → Apps. It can also add **Scan with Wield** to Explorer's right-click menu. |
| `wield-vX.Y.Z-windows-x64.exe` | **Portable.** One file, nothing to install. Run it from any folder or a USB stick. |

Both kinds update themselves (see [Settings and updates](settings.md)). Uninstalling keeps your Wield data (undo
histories and backups) in case you reinstall.

Windows SmartScreen may warn about a new download. Choose **More info → Run anyway** if you trust the source.
Each release also publishes a `.sha256` checksum for every file.

### The window

- **Sidebar (left).** **Home** shows every tool as a card. Below it is the list of tools; greyed-out tools are planned
  but not built yet.
- **Home cards** show a live one-line status for each tool, for example how many startup entries are enabled or when
  you last made a backup. Click a card to open the tool.
- **Bottom of the sidebar:**
  - **Light / Dark** switches between the Gruvbox light and dark themes. Your choice is remembered.
  - **Settings** opens updates, appearance, Explorer integration and data locations.
  - **Restart as admin** (when you're not an administrator) or the green **Administrator** label.
  - Your Windows edition, release and build.
  - **Update to vX.Y.Z** appears when a new version is available.
- **Messages** about finished actions pop up in the bottom-right corner (green for success, red for problems).

### PC health

The top of **Home** shows how the PC is doing, live. Nothing here goes online.

| Card | Shows |
|---|---|
| **Processor** | Load now and over the last two minutes, thread count and model. Also the **board sensor** temperature, the only one Windows shares without a driver. It's near the processor but isn't the exact core temperature, and many laptops only update it occasionally; Wield labels it **not updating** when the reading never changes. |
| **Memory** | Free memory, usage over the last two minutes, and how much is in use |
| **One card per drive** | Free space, with a bar that turns yellow below 12 % free and red below 5 % |
| **Graphics** | For NVIDIA cards: temperature, load and video memory (from NVIDIA's driver) |
| **Battery** or **Uptime** | Charge and whether it's plugged in; how long since the last restart and how many processes are running |

**Recommendations** below the cards suggest what would make the PC faster or safer, most urgent first. Each has a
button that opens the right Wield tool or Windows setting; **✕** hides one until Wield restarts.

| Recommendation | When | Button |
|---|---|---|
| Drive almost full / running low | Under 5 % (or 5 GB) / 12 % (or 15 GB) free | Storage settings |
| Temporary files | Over 1 GB in your Temp folder | **Clean temporary files**: deletes files there that haven't changed for over a day, skipping any in use. You confirm first. |
| Memory nearly full | Over 88 % used for half a minute | Process Explorer |
| Processor busy | Over 85 % for a minute | Process Explorer |
| Running hot | The board sensor (only if it updates) at 95 °C or more for several minutes while the processor is busy; graphics at 90 °C or more | Process Explorer |
| Restart to finish updates / not restarted for a week | Windows is waiting for a restart; or the PC has been up for over 7 days | Windows Update |
| Many apps start with Windows | 15 or more startup apps, or 3 or more with high impact | [Startup Manager](startup.md) |
| Devices with a driver problem | Any device with a problem code | [Driver Manager](drivers.md) |
| Real-time protection is off | Defender is your antivirus and isn't scanning files as they open | Windows Security |
| Power saver plan is on | While plugged in | Power settings |
| No backup yet | No successful backup made with Wield | [Backup & Restore](backup.md) |

CPU and memory refresh every 2 seconds, sensors and drives every 30 seconds, and the other checks every 10 minutes.

### Administrator rights

Wield starts as a normal user. Everything that only affects your account works straight away. Changes that affect
the whole PC need administrator rights, for example:

- services, all-users startup entries and scheduled tasks;
- PC-wide privacy settings;
- removing apps for all accounts;
- firewall rules, per-app traffic and repairs;
- Defender exclusions and quarantine;
- system images, restore points and recovery settings;
- deleting all-users right-click entries.

Screens that need it show an orange banner with a **Restart as administrator** button. Wield then reopens with a
Windows UAC prompt. Settings that need admin are marked with a lock icon.

### How Wield keeps you safe

Every tool follows the same rules:

- **Nothing changes without you clicking.** Opening a tool only reads.
- **Everything can be undone.** Disabling uses Windows' own reversible switches rather than deleting. When something
  must be deleted, an exact copy is saved first and can be restored from inside Wield.
- **Risk is shown up front.** Entries are rated (green / orange / red). Risky actions ask for confirmation, and
  dangerous ones make you type a word such as `DISABLE`, `REMOVE`, `RESTORE` or `DELETE`.
- **Checks before big jobs.** Backups, custom menu entries and similar tasks list their checks first and refuse to
  start when something would fail.
- **No hidden network traffic.** Wield sends nothing on its own except the update check, which you can turn off.
  Online lookups (malware reputation, public IP) only happen when you ask for them, and only fingerprints are sent,
  never your files.

## Where Wield keeps its data

Everything lives in `%LOCALAPPDATA%\Wield` (paste that into Explorer's address bar to open it):

| File or folder | Used by |
|---|---|
| `settings.json` | Theme and update preferences |
| `startup-journal.json` | Startup Manager change history (undo) |
| `privacy-backup.json` | Original values of every privacy setting Wield changed |
| `debloat-journal.json` | Removed apps, for reinstalling |
| `backup-history.json`, `backup-logs\` | Backup & Restore history and the full output of every backup |
| `restore-protection.json` | System Protection state as Wield last set it |
| `folder-backup.cmd` | The script run by the scheduled folder backup |
| `ctxmenu\` | Context Menu backups, deleted entries, hidden Send To items and your own entries |
| `app-installs\` | App Installer logs |
| `updates.json` | Apps you excluded from update checks |
| `hotkeys.json` | Shortcut keys you turned off in Keyboard Shortcuts, so they can be turned back on |

The hosts file backup is next to the hosts file: `C:\Windows\System32\drivers\etc\hosts.wield-backup`.

## Command-line options

| Option | Effect |
|---|---|
| `--tool <name>` | Opens a tool: `startup`, `privacy`, `debloat`, `network`, `malware`, `processes`, `apps`, `drivers`, `shortcuts`, `updates`, `backup`, `contextmenu` or `settings` |
| `--tab <name>` | Opens a tab. Network Toolkit: `activity`, `ports`, `firewall`, `scanner`, `tools`. Malware Scanner: `scan`, `protection`, `detections`. Backup & Restore: `image`, `restore`, `files`, `kit`. Context Menu: `menus`, `add`, `backups`, `win11`. Driver Manager: `overview`, `devices`, `problems`, `thirdparty`, `updates` (starts the check), `maker`. Keyboard Shortcuts: `windows`, `apps`, `inuse`. Update Control: `windows`, `apps`, `appscan` (starts the scan) |
| `--pid <number>` | Selects that process in Process Explorer |
| `--scan <file or folder>` | Opens the Malware Scanner and starts scanning it |
| `--run-hidden <command>` | Runs a command without a window, then exits. Used by right-click entries set to "Run hidden". |

Example: `wield.exe --tool network --tab firewall`

## Troubleshooting

**A switch or button is greyed out.** Hover over it: the tooltip says why. Usually it needs administrator rights
(use **Restart as administrator**) or another job is still running.

**A change didn't take effect.**

- Shell extensions and the Windows 11 menu setting need Explorer restarted. The Context Menu tool has a
  **Restart Explorer** button.
- Some network repairs need a Windows restart.
- System Restore is applied while Windows restarts.

**Windows blocks a change on a work PC.** Group Policy or Tamper Protection can lock settings, firewall rules and
Defender exclusions. Wield shows Windows' error message; ask your administrator.

**I want to undo something.**

| Tool | Where to undo |
|---|---|
| Startup Manager | **Undo previous changes** |
| Privacy & Telemetry | **Restore original settings** |
| App Debloater | **Removed apps → Reinstall** |
| Context Menu | **Backups & deleted** |
| Firewall rules | Switch off or remove them in **Network Toolkit → Firewall** |

You can also create a restore point before big changes ([Backup & Restore → System Restore](backup.md#system-restore)).

**Reporting a problem.** Use **Settings → Report a problem**, or open an issue on
[GitHub](https://github.com/plaintextlab/wield/issues). Include your Windows version from the bottom of the sidebar.
