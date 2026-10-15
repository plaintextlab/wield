//! What Wield knows about preinstalled and common Store apps, keyed by package name.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verdict {
    /// Preinstalled promotion or rarely used app: recommended to remove.
    Bloat,
    /// Fine to remove if you don't use it.
    Optional,
    /// Removing it breaks a Windows feature, hardware control panel or games.
    Caution,
    /// Removing it breaks Windows or app installation. Hard to undo.
    Keep,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Bloat => "Recommended to remove",
            Verdict::Optional => "Optional",
            Verdict::Caution => "Use caution",
            Verdict::Keep => "Keep",
        }
    }
}

use Verdict::{Bloat as B, Caution as C, Keep as K, Optional as O};

/// (package name prefix, lowercase), verdict, reason.
pub const APPS: &[(&str, Verdict, &str)] = &[
    // --- Keep: Windows or app installation depends on these
    ("microsoft.windowsstore", K, "Microsoft Store. Without it you can't install or update Store apps, and it can't easily be reinstalled."),
    ("microsoft.storepurchaseapp", K, "Store purchase flow. Needed to buy or install apps from the Store."),
    ("microsoft.desktopappinstaller", K, "App Installer and winget. Needed to install .msix/.appx files and for the winget command."),
    ("microsoft.sechealthui", K, "Windows Security app. You lose the UI for antivirus, firewall and device security."),
    ("microsoft.windows.shellexperiencehost", K, "Part of the Windows shell (Start, taskbar flyouts)."),
    ("microsoft.windows.startmenuexperiencehost", K, "The Start menu."),
    ("microsoftwindows.client", K, "Part of the Windows shell."),
    ("microsoft.ui.xaml", K, "UI framework used by many apps."),
    ("microsoft.vclibs", K, "C++ runtime used by many apps."),
    ("microsoft.net.native", K, ".NET runtime used by many apps."),
    ("microsoft.windowsappruntime", K, "Windows App SDK runtime used by many apps."),
    ("microsoftcorporationii.winappruntime", K, "Windows App SDK runtime. Apps built on it stop starting."),
    ("microsoft.microsoftedge", K, "Microsoft Edge. Windows uses it for help links, widgets and web content; removing it this way breaks those."),
    ("microsoft.office", C, "Component of Microsoft 365 (Office). Office features may stop working."),
    ("microsoft.officepushnotificationsutility", C, "Component of Microsoft 365 (Office) notifications."),
    ("aimgr", C, "Component of Microsoft 365 (Office) AI features."),
    ("microsoft.commandpalette", O, "PowerToys Command Palette."),
    ("microsoft.powertoys", O, "PowerToys."),
    ("microsoft.microsoftpcmanager", O, "Microsoft PC Manager cleanup utility."),
    ("microsoft.windows.photos", C, "Photos. The default image viewer; images will open in another app or not at all."),
    ("microsoft.windowscalculator", C, "Calculator."),
    ("microsoft.windowsnotepad", C, "Notepad (Store version). Text files will have no default editor."),
    ("microsoft.paint", C, "Paint."),
    ("microsoft.screensketch", C, "Snipping Tool. Win+Shift+S screenshots stop working."),
    ("microsoft.windowsterminal", C, "Windows Terminal."),
    ("microsoft.windows.devhome", O, "Dev Home. Developer dashboard; not needed by most people."),
    // Media codecs and extensions
    ("microsoft.heifimageextension", C, "HEIF image codec. Photos from iPhones and newer cameras won't open."),
    ("microsoft.heviclextension", C, "HEVC video codec."),
    ("microsoft.hevcvideoextension", C, "HEVC video codec. Many phone videos won't play."),
    ("microsoft.av1videoextension", C, "AV1 video codec."),
    ("microsoft.vp9videoextensions", C, "VP9 video codec, used by web video."),
    ("microsoft.webmediaextensions", C, "Web media codecs (Ogg, Vorbis, Theora)."),
    ("microsoft.webpimageextension", C, "WebP image codec. WebP images won't show thumbnails or open in Photos."),
    ("microsoft.rawimageextension", C, "RAW camera image codec."),
    ("microsoft.mpeg2videoextension", C, "MPEG-2 video codec."),
    // Xbox: needed for many PC games
    ("microsoft.xboxidentityprovider", C, "Xbox sign-in. Many PC games (Game Pass, Minecraft, Forza) need it to sign in."),
    ("microsoft.xbox.tcui", C, "Xbox UI components used by games for friends and invites."),
    ("microsoft.gamingservices", C, "Gaming Services. Required by Game Pass and Xbox PC games."),
    ("microsoft.xboxgamingoverlay", O, "Xbox Game Bar (Win+G): screen recording, performance overlay."),
    ("microsoft.xboxgameoverlay", O, "Legacy Xbox game overlay."),
    ("microsoft.xboxspeechtotextoverlay", B, "Xbox speech-to-text overlay for game chat."),
    ("microsoft.xboxapp", B, "Legacy Xbox Console Companion app."),
    ("microsoft.gamingapp", O, "Xbox app. Needed only for Game Pass on PC."),
    ("microsoft.edge.gameassist", B, "Edge Game Assist overlay."),
    // Hardware control panels
    ("appup.intelgraphicsexperience", C, "Intel Graphics Command Center. Display and graphics settings."),
    ("appup.intel", C, "Intel hardware utility."),
    ("realteksemiconductorcorp.realtekaudiocontrol", C, "Realtek Audio Console. Speaker, microphone and jack settings."),
    ("nvidiacorp.nvidiacontrolpanel", C, "NVIDIA Control Panel. Graphics, display and 3D settings."),
    ("advancedmicrodevicesinc-2.amdradeonsoftware", C, "AMD Radeon Software. Graphics settings and drivers."),
    ("synapticsincorporated", C, "Touchpad settings."),
    ("dolbylaboratories", C, "Dolby audio settings for your speakers."),
    ("dtsinc", C, "DTS audio settings for your speakers."),
    ("wavesaudio", C, "Waves audio settings for your speakers."),
    ("elanmicroelectronicscorpo", C, "Touchpad settings."),
    // Microsoft bloat and promotions
    ("microsoft.bingnews", B, "Microsoft News. Promotional news feed."),
    ("microsoft.bingweather", O, "Weather."),
    ("microsoft.bingfinance", B, "Money (discontinued)."),
    ("microsoft.bingsports", B, "Sports (discontinued)."),
    ("microsoft.bingsearch", B, "Bing Search app."),
    ("microsoft.gethelp", B, "Get Help. Points you to online support articles."),
    ("microsoft.getstarted", B, "Tips. Windows feature tips and promotions."),
    ("microsoft.microsoftsolitairecollection", B, "Solitaire Collection. Ad-supported games."),
    ("microsoft.microsoftofficehub", B, "Microsoft 365 (Office) hub. Promotes a subscription; Office apps work without it."),
    ("microsoft.office.onenote", O, "OneNote for Windows 10 (retired). The desktop OneNote is separate."),
    ("microsoft.people", B, "People (contacts) app."),
    ("microsoft.skypeapp", B, "Skype (retired by Microsoft)."),
    ("microsoft.windowsfeedbackhub", B, "Feedback Hub. Only needed to send feedback to Microsoft or for Insider builds."),
    ("microsoft.windowsmaps", B, "Maps (retired)."),
    ("microsoft.zunevideo", B, "Movies & TV. Store for renting films; videos open in Media Player instead."),
    ("microsoft.zunemusic", O, "Media Player / Groove Music. Remove only if you use another music/video player."),
    ("microsoft.mixedreality.portal", B, "Mixed Reality Portal. Only for Windows Mixed Reality headsets."),
    ("microsoft.microsoft3dviewer", B, "3D Viewer."),
    ("microsoft.print3d", B, "Print 3D."),
    ("microsoft.mspaint", B, "Paint 3D (retired)."),
    ("microsoft.oneconnect", B, "Mobile Plans. Buy mobile data plans."),
    ("microsoft.messaging", B, "Messaging (retired)."),
    ("microsoft.wallet", B, "Microsoft Pay (retired)."),
    ("microsoft.windows.ai.copilot", O, "Copilot."),
    ("microsoft.copilot", O, "Copilot."),
    ("microsoft.549981c3f5f10", B, "Cortana (retired)."),
    ("microsoft.todos", O, "Microsoft To Do."),
    ("microsoft.powerautomatedesktop", B, "Power Automate. Desktop automation; rarely used at home."),
    ("microsoft.microsoftstickynotes", O, "Sticky Notes."),
    ("microsoft.windowsalarms", O, "Clock (alarms, timers, focus sessions)."),
    ("microsoft.windowssoundrecorder", O, "Sound Recorder."),
    ("microsoft.windowscamera", O, "Camera app. Other apps can still use your camera."),
    ("microsoft.yourphone", O, "Phone Link. Connects an Android phone or iPhone to your PC."),
    ("microsoft.windowscommunicationsapps", O, "Mail and Calendar (being replaced by new Outlook)."),
    ("microsoft.outlookforwindows", O, "New Outlook for Windows."),
    ("microsoftteams", O, "Microsoft Teams (personal)."),
    ("msteams", O, "Microsoft Teams."),
    ("microsoft.microsoftjournal", B, "Journal. Pen note-taking app."),
    ("microsoftcorporationii.microsoftfamily", B, "Microsoft Family Safety."),
    ("microsoftcorporationii.quickassist", O, "Quick Assist. Lets someone help you remotely; scammers abuse it."),
    ("microsoftwindows.crossdevice", O, "Cross Device Experience Host (Phone Link integration)."),
    ("clipchamp.clipchamp", B, "Clipchamp video editor."),
    // Third-party preinstalled promotions
    ("spotifyab.spotifymusic", B, "Spotify (preinstalled promotion)."),
    ("disney.", B, "Disney+ (preinstalled promotion)."),
    ("king.com.", B, "King games such as Candy Crush (preinstalled promotion)."),
    ("facebook.", B, "Facebook/Instagram/Messenger (preinstalled promotion)."),
    ("4df9e0f8.netflix", B, "Netflix (preinstalled promotion)."),
    ("bytedancepte.ltd.tiktok", B, "TikTok (preinstalled promotion)."),
    ("amazonvideo.primevideo", B, "Prime Video (preinstalled promotion)."),
    ("7ee7776c.linkedinforwindows", B, "LinkedIn (preinstalled promotion)."),
    ("9e2f88e3.twitter", B, "X / Twitter (preinstalled promotion)."),
    ("clearchannelradiodigital.iheartradio", B, "iHeartRadio (preinstalled promotion)."),
    ("adobesystemsincorporated.adobephotoshopexpress", B, "Photoshop Express (preinstalled promotion)."),
    ("pandoramediainc", B, "Pandora (preinstalled promotion)."),
    ("5a894077.mcafeesecurity", B, "McAfee trial (preinstalled promotion)."),
    ("a278ab0d.marchofempires", B, "March of Empires (preinstalled game)."),
    ("a278ab0d.dragonmanialegends", B, "Dragon Mania Legends (preinstalled game)."),
    ("2fe3cb00.picsart", B, "PicsArt (preinstalled promotion)."),
    ("shazamentertainmentltd.shazam", B, "Shazam (preinstalled promotion)."),
    ("flipboard.flipboard", B, "Flipboard (preinstalled promotion)."),
    ("duolingo", B, "Duolingo (preinstalled promotion)."),
    ("e046963f.lenovocompanion", O, "Lenovo Vantage. Updates, battery and warranty settings for Lenovo PCs."),
    ("ad2f1837.", O, "HP utility app."),
    ("dellinc.", O, "Dell utility app."),
    ("acerincorporated.", O, "Acer utility app."),
    ("b9ecED6F.", O, "ASUS utility app."),
];

/// Looks up a package by its name (`Publisher.AppName`). Longest matching prefix wins.
pub fn lookup(package_name: &str) -> Option<(Verdict, &'static str)> {
    let n = package_name.to_lowercase();
    APPS.iter()
        .filter(|(p, ..)| n.starts_with(&p.to_lowercase()))
        .max_by_key(|(p, ..)| p.len())
        .map(|(_, v, r)| (*v, *r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_apps() {
        assert_eq!(lookup("Microsoft.BingNews").unwrap().0, Verdict::Bloat);
        assert_eq!(lookup("Microsoft.WindowsStore").unwrap().0, Verdict::Keep);
        assert_eq!(lookup("king.com.CandyCrushSaga").unwrap().0, Verdict::Bloat);
        assert!(lookup("Contoso.Unknown").is_none());
    }
}
