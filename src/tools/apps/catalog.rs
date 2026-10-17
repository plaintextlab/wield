//! The app catalog: free, widely trusted software, installed from the official winget repository.
//! No trials or paid software, except DaVinci Resolve Studio (opens its official page).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cat {
    Browsers,
    Media,
    Creative,
    Gaming,
    Runtimes,
    Hardware,
    Utilities,
    Communication,
    Developer,
}

impl Cat {
    pub const ALL: [Cat; 9] = [Cat::Utilities, Cat::Browsers, Cat::Media, Cat::Creative, Cat::Gaming, Cat::Runtimes, Cat::Hardware, Cat::Communication, Cat::Developer];
    pub fn label(self) -> &'static str {
        match self {
            Cat::Browsers => "Web browsers",
            Cat::Media => "Music & video",
            Cat::Creative => "Photo, video & streaming",
            Cat::Gaming => "Games & launchers",
            Cat::Runtimes => "Game & app runtimes",
            Cat::Hardware => "Hardware & monitoring",
            Cat::Utilities => "Essential utilities",
            Cat::Communication => "Chat & email",
            Cat::Developer => "Developer tools",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Cat::Runtimes => "Libraries many games and programs need. If a game says a DLL is missing, install these.",
            Cat::Hardware => "See temperatures, clocks and disk health; tune and benchmark.",
            _ => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// One or more winget package IDs, installed in order.
    Winget(&'static [&'static str]),
    /// Not in winget (registration or purchase needed): opens the official download page.
    Website(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct App {
    pub key: &'static str,
    pub name: &'static str,
    pub cat: Cat,
    pub blurb: &'static str,
    pub source: Source,
    /// Start of the program's name in Windows' installed-programs list, to show it as installed.
    pub detect: &'static [&'static str],
}

const fn w(ids: &'static [&'static str]) -> Source {
    Source::Winget(ids)
}

macro_rules! app {
    ($key:literal, $name:literal, $cat:ident, $blurb:literal, $source:expr, [$($d:literal),*]) => {
        App { key: $key, name: $name, cat: Cat::$cat, blurb: $blurb, source: $source, detect: &[$($d),*] }
    };
}

pub const APPS: &[App] = &[
    // Essential utilities
    app!("7zip", "7-Zip", Utilities, "Open and create ZIP, 7z, RAR and other archives.", w(&["7zip.7zip"]), ["7-Zip"]),
    app!("powertoys", "Microsoft PowerToys", Utilities, "FancyZones, PowerRename, color picker, text extractor and 20 more tools.", w(&["Microsoft.PowerToys"]), ["PowerToys"]),
    app!("everything", "Everything", Utilities, "Finds any file on your PC instantly as you type.", w(&["voidtools.Everything"]), ["Everything"]),
    app!("notepadpp", "Notepad++", Utilities, "Fast text and code editor with tabs and syntax highlighting.", w(&["Notepad++.Notepad++"]), ["Notepad++"]),
    app!("sharex", "ShareX", Utilities, "Screenshots, screen recording, annotation and OCR.", w(&["ShareX.ShareX"]), ["ShareX"]),
    app!("sumatra", "SumatraPDF", Utilities, "Small, fast PDF, ePub and comic reader.", w(&["SumatraPDF.SumatraPDF"]), ["SumatraPDF"]),
    app!("acrobat", "Adobe Acrobat Reader", Utilities, "The standard PDF reader, with form filling and signing.", w(&["Adobe.Acrobat.Reader.64-bit"]), ["Adobe Acrobat"]),
    app!("libreoffice", "LibreOffice", Utilities, "Free office suite: documents, spreadsheets, presentations. Opens Word and Excel files.", w(&["TheDocumentFoundation.LibreOffice"]), ["LibreOffice"]),
    app!("wiztree", "WizTree", Utilities, "Shows what's using your disk space, in seconds.", w(&["AntibodySoftware.WizTree"]), ["WizTree"]),
    app!("keepassxc", "KeePassXC", Utilities, "Offline password manager.", w(&["KeePassXCTeam.KeePassXC"]), ["KeePassXC"]),
    app!("bitwarden", "Bitwarden", Utilities, "Password manager that syncs across devices.", w(&["Bitwarden.Bitwarden"]), ["Bitwarden"]),
    app!("rufus", "Rufus", Utilities, "Make bootable USB drives from Windows or Linux ISO files.", w(&["Rufus.Rufus"]), ["Rufus"]),
    app!("ventoy", "Ventoy", Utilities, "Put many ISO files on one bootable USB stick.", w(&["Ventoy.Ventoy"]), ["Ventoy"]),
    app!("qbittorrent", "qBittorrent", Utilities, "Ad-free torrent client.", w(&["qBittorrent.qBittorrent"]), ["qBittorrent"]),
    app!("localsend", "LocalSend", Utilities, "Send files to nearby phones and PCs over Wi-Fi, like AirDrop.", w(&["LocalSend.LocalSend"]), ["LocalSend"]),
    app!("bleachbit", "BleachBit", Utilities, "Clears caches and temporary files.", w(&["BleachBit.BleachBit"]), ["BleachBit"]),
    app!("flowlauncher", "Flow Launcher", Utilities, "Quick launcher: open apps, files and web searches from the keyboard.", w(&["Flow-Launcher.Flow-Launcher"]), ["Flow Launcher"]),
    app!("autohotkey", "AutoHotkey", Utilities, "Automate keystrokes and create your own shortcuts.", w(&["AutoHotkey.AutoHotkey"]), ["AutoHotkey"]),
    // Browsers
    app!("firefox", "Mozilla Firefox", Browsers, "Private, independent browser with strong tracking protection.", w(&["Mozilla.Firefox"]), ["Mozilla Firefox"]),
    app!("chrome", "Google Chrome", Browsers, "Google's browser.", w(&["Google.Chrome"]), ["Google Chrome"]),
    app!("brave", "Brave", Browsers, "Chromium-based browser with built-in ad blocking.", w(&["Brave.Brave"]), ["Brave"]),
    // Music & video
    app!("vlc", "VLC media player", Media, "Plays every video and audio format, no codecs needed.", w(&["VideoLAN.VLC"]), ["VLC media player"]),
    app!("mpchc", "MPC-HC", Media, "Light, classic video player.", w(&["clsid2.mpc-hc"]), ["MPC-HC"]),
    app!("klite", "K-Lite Codec Pack", Media, "Codecs so every player (including Windows' own) can play every format.", w(&["CodecGuide.K-LiteCodecPack.Standard"]), ["K-Lite Codec Pack"]),
    app!("foobar", "foobar2000", Media, "Lightweight, highly customizable music player.", w(&["PeterPawlowski.foobar2000"]), ["foobar2000"]),
    app!("aimp", "AIMP", Media, "Feature-rich music player with an equalizer and skins.", w(&["AIMP.AIMP"]), ["AIMP"]),
    app!("spotify", "Spotify", Media, "Music streaming (free with ads, or with your subscription).", w(&["Spotify.Spotify"]), ["Spotify"]),
    app!("handbrake", "HandBrake", Media, "Convert and compress videos.", w(&["HandBrake.HandBrake"]), ["HandBrake"]),
    app!("mediainfo", "MediaInfo", Media, "Shows the codec, resolution and bitrate of any media file.", w(&["MediaArea.MediaInfo.GUI"]), ["MediaInfo"]),
    app!("ffmpeg", "FFmpeg", Media, "Command-line tool to convert any audio or video.", w(&["Gyan.FFmpeg"]), []),
    app!("imageglass", "ImageGlass", Media, "Fast, modern image viewer.", w(&["DuongDieuPhap.ImageGlass"]), ["ImageGlass"]),
    app!("irfanview", "IrfanView", Media, "Classic, very fast image viewer and batch converter.", w(&["IrfanSkiljan.IrfanView"]), ["IrfanView"]),
    // Photo, video & streaming
    app!("obs", "OBS Studio", Creative, "Record your screen and stream to Twitch or YouTube.", w(&["OBSProject.OBSStudio"]), ["OBS Studio"]),
    app!("gimp", "GIMP", Creative, "Full photo editor, a free alternative to Photoshop.", w(&["GIMP.GIMP.3"]), ["GIMP"]),
    app!("krita", "Krita", Creative, "Digital painting and illustration.", w(&["KDE.Krita"]), ["Krita"]),
    app!("inkscape", "Inkscape", Creative, "Vector graphics, a free alternative to Illustrator.", w(&["Inkscape.Inkscape"]), ["Inkscape"]),
    app!("paintnet", "Paint.NET", Creative, "Easy photo and image editor.", w(&["dotPDN.PaintDotNet"]), ["paint.net"]),
    app!("kdenlive", "Kdenlive", Creative, "Video editor with multitrack timeline and effects.", w(&["KDE.Kdenlive"]), ["Kdenlive"]),
    app!("shotcut", "Shotcut", Creative, "Simple, capable video editor.", w(&["Meltytech.Shotcut"]), ["Shotcut"]),
    app!(
        "resolve",
        "DaVinci Resolve",
        Creative,
        "Professional video editing, color and audio. Free version. Blackmagic asks you to register, so this opens its download page.",
        Source::Website("https://www.blackmagicdesign.com/products/davinciresolve"),
        ["DaVinci Resolve"]
    ),
    app!(
        "resolvestudio",
        "DaVinci Resolve Studio",
        Creative,
        "Paid version of DaVinci Resolve (license required). Opens Blackmagic's download page.",
        Source::Website("https://www.blackmagicdesign.com/products/davinciresolve/studio"),
        []
    ),
    app!("blender", "Blender", Creative, "3D modeling, animation and rendering.", w(&["BlenderFoundation.Blender"]), ["blender"]),
    app!("audacity", "Audacity", Creative, "Record and edit audio.", w(&["Audacity.Audacity"]), ["Audacity"]),
    // Games & launchers
    app!("steam", "Steam", Gaming, "The biggest PC game store and launcher.", w(&["Valve.Steam"]), ["Steam"]),
    app!("epic", "Epic Games Launcher", Gaming, "Epic Games Store, Fortnite, Unreal Engine and weekly free games.", w(&["EpicGames.EpicGamesLauncher"]), ["Epic Games Launcher"]),
    app!("ubisoft", "Ubisoft Connect", Gaming, "Launcher for Ubisoft games (Assassin's Creed, Far Cry…).", w(&["Ubisoft.Connect"]), ["Ubisoft Connect"]),
    app!("ea", "EA app", Gaming, "Launcher for EA games (EA Sports FC, Battlefield, The Sims…).", w(&["ElectronicArts.EADesktop"]), ["EA app"]),
    app!("gog", "GOG Galaxy", Gaming, "GOG's DRM-free store, and one library for all your launchers.", w(&["GOG.Galaxy"]), ["GOG GALAXY"]),
    app!("battlenet", "Battle.net", Gaming, "Blizzard and Activision games (Diablo, Overwatch, Call of Duty).", w(&["Blizzard.BattleNet"]), ["Battle.net"]),
    app!("amazongames", "Amazon Games", Gaming, "Prime Gaming free games.", w(&["Amazon.Games"]), ["Amazon Games"]),
    app!("itch", "itch", Gaming, "Indie games from itch.io.", w(&["ItchIo.Itch"]), ["itch"]),
    app!("heroic", "Heroic Games Launcher", Gaming, "Open-source launcher for Epic, GOG and Amazon games.", w(&["HeroicGamesLauncher.HeroicGamesLauncher"]), ["Heroic"]),
    app!("playnite", "Playnite", Gaming, "One library for games from every launcher.", w(&["Playnite.Playnite"]), ["Playnite"]),
    app!("minecraft", "Minecraft Launcher", Gaming, "Official launcher for Minecraft Java and Bedrock.", w(&["Mojang.MinecraftLauncher"]), ["Minecraft Launcher"]),
    app!("prism", "Prism Launcher", Gaming, "Minecraft launcher for mods and multiple instances.", w(&["PrismLauncher.PrismLauncher"]), ["Prism Launcher"]),
    app!("vortex", "Vortex", Gaming, "Nexus Mods' mod manager.", w(&["NexusMods.Vortex"]), ["Vortex"]),
    // Runtimes
    app!("vcredist", "Visual C++ Redistributable 2015–2022", Runtimes, "Needed by most modern games and programs (64- and 32-bit).", w(&["Microsoft.VCRedist.2015+.x64", "Microsoft.VCRedist.2015+.x86"]), ["Microsoft Visual C++ v14", "Microsoft Visual C++ 2015-", "Microsoft Visual C++ 2022"]),
    app!(
        "vcredistold",
        "Visual C++ Redistributables 2005–2013",
        Runtimes,
        "Older versions many older games still need (64- and 32-bit).",
        w(&[
            "Microsoft.VCRedist.2013.x64",
            "Microsoft.VCRedist.2013.x86",
            "Microsoft.VCRedist.2012.x64",
            "Microsoft.VCRedist.2012.x86",
            "Microsoft.VCRedist.2010.x64",
            "Microsoft.VCRedist.2010.x86",
            "Microsoft.VCRedist.2008.x64",
            "Microsoft.VCRedist.2008.x86",
            "Microsoft.VCRedist.2005.x64",
            "Microsoft.VCRedist.2005.x86"
        ]),
        ["Microsoft Visual C++ 2013"]
    ),
    app!("directx", "DirectX End-User Runtime", Runtimes, "Older DirectX 9–11 files (d3dx9, xinput1_3…) that many games need.", w(&["Microsoft.DirectX"]), []),
    app!("dotnet8", ".NET Desktop Runtime 8", Runtimes, "Needed by many desktop apps and mod tools.", w(&["Microsoft.DotNet.DesktopRuntime.8"]), ["Microsoft Windows Desktop Runtime - 8"]),
    app!("dotnet10", ".NET Desktop Runtime 10", Runtimes, "The newest .NET runtime.", w(&["Microsoft.DotNet.DesktopRuntime.10"]), ["Microsoft Windows Desktop Runtime - 10"]),
    app!("xna", "XNA Framework 4.0", Runtimes, "Needed by older indie games (Terraria, Stardew Valley's older versions…).", w(&["Microsoft.XNARedist"]), ["Microsoft XNA Framework"]),
    app!("physx", "NVIDIA PhysX", Runtimes, "Physics engine some games need (also works without an NVIDIA card).", w(&["Nvidia.PhysX"]), ["NVIDIA PhysX"]),
    app!("java", "Java 21 (Temurin JRE)", Runtimes, "Runs Java programs and Minecraft mods.", w(&["EclipseAdoptium.Temurin.21.JRE"]), ["Eclipse Temurin JRE"]),
    // Hardware & monitoring
    app!("nvidiaapp", "NVIDIA App", Hardware, "NVIDIA graphics drivers and settings. Not in winget, so this opens NVIDIA's download page.", Source::Website("https://www.nvidia.com/en-us/software/nvidia-app/"), ["NVIDIA App"]),
    app!("amd", "AMD Software: Adrenalin", Hardware, "AMD Radeon drivers and settings. Opens AMD's download page.", Source::Website("https://www.amd.com/en/support/download/drivers.html"), ["AMD Software"]),
    app!("intel", "Intel Driver & Support Assistant", Hardware, "Keeps Intel graphics, Wi-Fi and chipset drivers up to date.", w(&["Intel.IntelDriverAndSupportAssistant"]), ["Intel® Driver & Support Assistant", "Intel Driver"]),
    app!("afterburner", "MSI Afterburner", Hardware, "GPU overclocking, fan curves and in-game FPS overlay (with RTSS).", w(&["Guru3D.Afterburner"]), ["MSI Afterburner"]),
    app!("rtss", "RivaTuner Statistics Server", Hardware, "FPS counter, overlay and frame limiter.", w(&["Guru3D.RTSS"]), ["RivaTuner"]),
    app!("hwinfo", "HWiNFO", Hardware, "Every sensor: temperatures, clocks, voltages, fan speeds.", w(&["REALiX.HWiNFO"]), ["HWiNFO"]),
    app!("cpuz", "CPU-Z", Hardware, "Processor, memory and motherboard details.", w(&["CPUID.CPU-Z"]), ["CPUID CPU-Z"]),
    app!("gpuz", "GPU-Z", Hardware, "Graphics card details and sensors.", w(&["TechPowerUp.GPU-Z"]), ["TechPowerUp GPU-Z", "GPU-Z"]),
    app!("diskinfo", "CrystalDiskInfo", Hardware, "Disk health (S.M.A.R.T.) and temperature. Warns before a drive fails.", w(&["CrystalDewWorld.CrystalDiskInfo"]), ["CrystalDiskInfo"]),
    app!("diskmark", "CrystalDiskMark", Hardware, "Measure how fast your drives are.", w(&["CrystalDewWorld.CrystalDiskMark"]), ["CrystalDiskMark"]),
    // Chat & email
    app!("discord", "Discord", Communication, "Voice and text chat for gamers and communities.", w(&["Discord.Discord"]), ["Discord"]),
    app!("telegram", "Telegram", Communication, "Fast messaging with desktop sync.", w(&["Telegram.TelegramDesktop"]), ["Telegram Desktop"]),
    app!("thunderbird", "Thunderbird", Communication, "Email, calendar and contacts.", w(&["Mozilla.Thunderbird"]), ["Mozilla Thunderbird"]),
    // Developer
    app!("git", "Git", Developer, "Version control (includes Git Bash).", w(&["Git.Git"]), ["Git"]),
    app!("vscode", "Visual Studio Code", Developer, "Code editor with extensions for every language.", w(&["Microsoft.VisualStudioCode"]), ["Microsoft Visual Studio Code"]),
    app!("githubdesktop", "GitHub Desktop", Developer, "Simple Git client for GitHub.", w(&["GitHub.GitHubDesktop"]), ["GitHub Desktop"]),
    app!("terminal", "Windows Terminal", Developer, "Modern terminal with tabs for PowerShell, cmd and WSL.", w(&["Microsoft.WindowsTerminal"]), ["Windows Terminal"]),
    app!("pwsh", "PowerShell 7", Developer, "The current, cross-platform PowerShell.", w(&["Microsoft.PowerShell"]), ["PowerShell 7"]),
    app!("python", "Python 3", Developer, "Python 3.14 with pip.", w(&["Python.Python.3.14"]), ["Python 3.14"]),
    app!("node", "Node.js LTS", Developer, "JavaScript runtime with npm.", w(&["OpenJS.NodeJS.LTS"]), ["Node.js"]),
    app!("vs", "Visual Studio Community", Developer, "Microsoft's full IDE for C++, C# and .NET (free for individuals).", w(&["Microsoft.VisualStudio.Community"]), ["Visual Studio Community"]),
    app!("docker", "Docker Desktop", Developer, "Containers on Windows (free for personal use and small businesses).", w(&["Docker.DockerDesktop"]), ["Docker Desktop"]),
    app!("winmerge", "WinMerge", Developer, "Compare and merge files and folders.", w(&["WinMerge.WinMerge"]), ["WinMerge"]),
    app!("sysinternals", "Sysinternals Suite", Developer, "Microsoft's advanced system tools (Process Monitor, Autoruns…).", w(&["Microsoft.Sysinternals.Suite"]), []),
    app!("winscp", "WinSCP", Developer, "SFTP, FTP and SCP file transfer.", w(&["WinSCP.WinSCP"]), ["WinSCP"]),
    app!("putty", "PuTTY", Developer, "SSH and telnet client.", w(&["PuTTY.PuTTY"]), ["PuTTY"]),
    app!("wireshark", "Wireshark", Developer, "Capture and inspect network traffic.", w(&["WiresharkFoundation.Wireshark"]), ["Wireshark"]),
];

/// One-click selections.
pub const PRESETS: &[(&str, &str, &[&str])] = &[
    ("Essentials", "The basics most PCs should have", &["7zip", "vlc", "notepadpp", "powertoys", "everything", "sharex", "sumatra", "firefox", "vcredist"]),
    ("Gaming", "Launchers, runtimes and an FPS overlay", &["steam", "epic", "ubisoft", "ea", "gog", "battlenet", "discord", "vcredist", "vcredistold", "directx", "dotnet8", "xna", "physx", "afterburner", "rtss"]),
    ("Creator", "Photo, video, audio and streaming", &["obs", "gimp", "krita", "inkscape", "kdenlive", "resolve", "audacity", "handbrake", "blender"]),
    ("Developer", "Code, Git and terminals", &["git", "vscode", "terminal", "pwsh", "python", "node", "githubdesktop", "winmerge"]),
];

pub fn find(key: &str) -> Option<&'static App> {
    APPS.iter().find(|a| a.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        let mut keys: Vec<&str> = APPS.iter().map(|a| a.key).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate keys");
        for (_, _, items) in PRESETS {
            for k in *items {
                assert!(find(k).is_some(), "preset refers to unknown app {k}");
            }
        }
        for a in APPS {
            if let Source::Winget(ids) = a.source {
                assert!(!ids.is_empty() && ids.iter().all(|i| i.contains('.') && !i.contains(' ')), "{}", a.key);
            }
        }
        for c in Cat::ALL {
            assert!(APPS.iter().any(|a| a.cat == c), "empty category {c:?}");
        }
    }
}
