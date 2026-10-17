# App Installer

[← Manual](README.md)

Set up a PC in minutes: tick the apps you want and install them all at once, silently, from their official sources.
Every app in the list is free. The one exception is DaVinci Resolve Studio, which opens Blackmagic's page.

![App Installer](images/apps.png)

## How it works

Wield uses the **Windows Package Manager (winget)**, Microsoft's official installer tool. Each app is downloaded
from the publisher's own servers, through Microsoft's curated, checksum-verified package catalog. Wield never
bundles or repackages anything.

### If winget is missing

Windows 11 and recent Windows 10 include winget. **LTSC/IoT editions and older Windows 10 builds don't.** Wield then
shows an orange banner with **Install winget**:

- Wield downloads App Installer and its dependencies (about 300 MB) from Microsoft's official
  [winget-cli release](https://github.com/microsoft/winget-cli/releases) on GitHub.
- Every file is checked against the SHA-256 checksum GitHub publishes for it.
- It's installed for your account and needs no administrator rights.

## Choosing apps

There are 91 apps in nine categories. Click a tile to tick or untick it.

| Category | Examples |
|---|---|
| **Essential utilities** | 7-Zip, Microsoft PowerToys, Everything, Notepad++, ShareX, SumatraPDF, Adobe Acrobat Reader, LibreOffice, WizTree, KeePassXC, Bitwarden, Rufus, Ventoy, qBittorrent, LocalSend, BleachBit, Flow Launcher, AutoHotkey |
| **Web browsers** | Firefox, Chrome, Brave |
| **Music & video** | VLC, MPC-HC, K-Lite Codec Pack, foobar2000, AIMP, Spotify, HandBrake, MediaInfo, FFmpeg, ImageGlass, IrfanView |
| **Photo, video & streaming** | OBS Studio, GIMP, Krita, Inkscape, Paint.NET, Kdenlive, Shotcut, DaVinci Resolve (free and Studio), Blender, Audacity |
| **Games & launchers** | Steam, Epic Games Launcher, Ubisoft Connect, EA app, GOG Galaxy, Battle.net, Amazon Games, itch, Heroic, Playnite, Minecraft Launcher, Prism Launcher, Vortex |
| **Game & app runtimes** | Visual C++ Redistributables (2005–2022, 64- and 32-bit), DirectX End-User Runtime, .NET Desktop Runtime 8 and 10, XNA Framework, NVIDIA PhysX, Java 21 |
| **Hardware & monitoring** | NVIDIA App, AMD Software: Adrenalin, Intel Driver & Support Assistant, MSI Afterburner, RivaTuner Statistics Server, HWiNFO, CPU-Z, GPU-Z, CrystalDiskInfo, CrystalDiskMark |
| **Chat & email** | Discord, Telegram, Thunderbird |
| **Developer tools** | Git, Visual Studio Code, GitHub Desktop, Windows Terminal, PowerShell 7, Python 3, Node.js LTS, Visual Studio Community, Docker Desktop, WinMerge, Sysinternals Suite, WinSCP, PuTTY, Wireshark |

On each tile:

- **Installed** (green) means the app is already on this PC. Wield checks winget's list and Windows'
  installed-programs list, so apps installed any other way are recognized too.
- **Website** (blue) marks apps that can't be installed silently, because the publisher requires registration or a
  license. These are DaVinci Resolve, DaVinci Resolve Studio, NVIDIA App and AMD Adrenalin. Installing them opens
  the official download page in your browser.
- Hover a tile to see its winget package ID.

**Quick picks** tick a ready-made set (apps already installed are skipped). Hover a button to see what it includes.

| Quick pick | Apps |
|---|---|
| **Essentials** | 7-Zip, VLC, Notepad++, PowerToys, Everything, ShareX, SumatraPDF, Firefox, Visual C++ |
| **Gaming** | Steam, Epic, Ubisoft Connect, EA app, GOG Galaxy, Battle.net, Discord, all game runtimes, MSI Afterburner, RTSS |
| **Creator** | OBS Studio, GIMP, Krita, Inkscape, Kdenlive, DaVinci Resolve, Audacity, HandBrake, Blender |
| **Developer** | Git, VS Code, Windows Terminal, PowerShell 7, Python, Node.js, GitHub Desktop, WinMerge |

You can also:

- **Select all** next to a category: ticks everything in it that isn't installed yet.
- **Search apps**: filters by name or description.
- **Hide installed**: shows only what you don't have yet.
- **Clear selection**.

## Installing

1. Tick the apps and click **Install N apps** (top right).
2. The confirmation lists the apps, notes any already installed (they're updated if a newer version exists) and any
   that open a website. Click **Install**. By installing, you accept each app's license.
3. The progress card shows each app's status, the current download percentage and the live output (**Details**).
   You can keep using your PC.
4. **Stop after this app** ends the queue cleanly after the current installation.

Each app ends with one of these results:

| Result | Meaning |
|---|---|
| ✅ **Installed** | Installed successfully |
| ✅ **Already installed and up to date** | Nothing to do |
| 🔄 **Installed; restart Windows to finish** | Restart when convenient |
| 🌐 **Download page opened** | Finish the download on the publisher's site |
| ❌ **Error** | With the reason, e.g. the app is running, the permission prompt was declined, or no installer exists for this PC |

Apps that installed are unticked; failed ones stay ticked so you can retry. **Log** opens the full output, saved in
`%LOCALAPPDATA%\Wield\app-installs\`.

### Administrator prompts

Many installers install for all users and need permission. If Wield isn't running as administrator, Windows asks
for each such app. To install everything without prompts, click **Restart as administrator** in the blue banner
first.

## Updating apps

**Update all apps** updates every installed app winget knows about, including apps you didn't install with Wield.
It runs silently and shows its progress the same way.

## Tips

- Install the **Game & app runtimes** on a new PC before your games. They fix most "missing DLL" errors
  (`MSVCP140.dll`, `d3dx9_43.dll`, `xinput1_3.dll`…).
- After a clean Windows install, a [reinstall kit](backup.md#reinstall-kit) can also reinstall your previous apps
  with `winget import`.
