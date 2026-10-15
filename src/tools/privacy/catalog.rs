//! Every privacy setting Wield knows how to change.
//!
//! A *tweak* is one user-facing switch made of one or more concrete *changes*. "On" means the
//! privacy-friendly value is applied; "off" restores the Windows default. Policy values default to
//! "absent", which is how Windows ships.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    User,
    Machine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Data {
    Dword(u32),
    Str(&'static str),
    /// Lowest `AllowTelemetry` level the edition honors: 0 on Enterprise/Education/IoT/Server, 1 elsewhere.
    TelemetryMin,
}

#[derive(Clone, Copy, Debug)]
pub enum Change {
    Reg { hive: Hive, path: &'static str, name: &'static str, private: Data, default: Option<Data> },
    Service { name: &'static str, private_start: u32, default_start: u32, default_delayed: bool },
    Tasks(&'static [&'static str]),
}

impl Change {
    pub fn needs_admin(&self) -> bool {
        !matches!(self, Change::Reg { hive: Hive::User, .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Telemetry,
    Ads,
    Search,
    Activity,
    Ai,
    Permissions,
}

impl Category {
    pub const ALL: [Category; 6] =
        [Category::Telemetry, Category::Ads, Category::Search, Category::Activity, Category::Ai, Category::Permissions];

    pub fn label(self) -> &'static str {
        match self {
            Category::Telemetry => "Telemetry & diagnostics",
            Category::Ads => "Ads & suggestions",
            Category::Search => "Search",
            Category::Activity => "Activity, speech & typing",
            Category::Ai => "AI features",
            Category::Permissions => "App permissions",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Category::Telemetry => "What Windows reports back to Microsoft about your device and how you use it.",
            Category::Ads => "Advertising ID, promoted apps, tips and other marketing inside Windows.",
            Category::Search => "Web results, Bing and cloud content in Start and taskbar search.",
            Category::Activity => "Activity history, clipboard sync, online speech and typing personalization.",
            Category::Ai => "Copilot and Recall on newer Windows 11 builds.",
            Category::Permissions => {
                "Which Store apps may use sensitive data. Desktop programs are not affected. Only block what you don't use."
            }
        }
    }
}

#[derive(Debug)]
pub struct Tweak {
    pub id: &'static str,
    pub category: Category,
    pub title: &'static str,
    pub summary: &'static str,
    /// What you give up when the protection is on.
    pub tradeoff: &'static str,
    pub recommended: bool,
    /// Minimum Windows build where it applies (0 = all).
    pub min_build: u32,
    pub note: Option<&'static str>,
    pub changes: &'static [Change],
}

impl Tweak {
    pub fn needs_admin(&self) -> bool {
        self.changes.iter().any(Change::needs_admin)
    }
}

use Change::{Reg, Service, Tasks};
use Data::{Dword, Str};
use Hive::{Machine as M, User as U};

const DATA_COLLECTION: &str = r"SOFTWARE\Policies\Microsoft\Windows\DataCollection";
const CDM: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
const EXPLORER_ADV: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const SEARCH_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\SearchSettings";
const SYSTEM_POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows\System";
const CLOUD_CONTENT_USER: &str = r"Software\Policies\Microsoft\Windows\CloudContent";

const fn cdm(name: &'static str) -> Change {
    Reg { hive: U, path: CDM, name, private: Dword(0), default: Some(Dword(1)) }
}

const fn consent(path: &'static str) -> Change {
    Reg { hive: U, path, name: "Value", private: Str("Deny"), default: Some(Str("Allow")) }
}

pub const TELEMETRY_TASKS: &[&str] = &[
    r"\Microsoft\Windows\Application Experience\Microsoft Compatibility Appraiser",
    r"\Microsoft\Windows\Application Experience\ProgramDataUpdater",
    r"\Microsoft\Windows\Application Experience\PcaPatchDbTask",
    r"\Microsoft\Windows\Customer Experience Improvement Program\Consolidator",
    r"\Microsoft\Windows\Customer Experience Improvement Program\UsbCeip",
    r"\Microsoft\Windows\DiskDiagnostic\Microsoft-Windows-DiskDiagnosticDataCollector",
    r"\Microsoft\Windows\Feedback\Siuf\DmClient",
    r"\Microsoft\Windows\Feedback\Siuf\DmClientOnScenarioDownload",
    r"\Microsoft\Windows\Autochk\Proxy",
];

pub const TWEAKS: &[Tweak] = &[
    // ---------------------------------------------------------------- Telemetry
    Tweak {
        id: "diag_level",
        category: Category::Telemetry,
        title: "Send the minimum diagnostic data",
        summary: "Limits the diagnostic data Windows sends to Microsoft to the lowest level your edition allows.",
        tradeoff: "Some Windows Insider and troubleshooting features need optional diagnostic data.",
        recommended: true,
        min_build: 0,
        note: Some("Home and Pro can go down to \"Required\". Only Enterprise, Education and LTSC/IoT can turn it off completely (\"Security\")."),
        changes: &[Reg { hive: M, path: DATA_COLLECTION, name: "AllowTelemetry", private: Data::TelemetryMin, default: None }],
    },
    Tweak {
        id: "diagtrack",
        category: Category::Telemetry,
        title: "Turn off the telemetry service",
        summary: "Disables \"Connected User Experiences and Telemetry\" (DiagTrack), the service that uploads diagnostic data.",
        tradeoff: "Windows Insider builds need it. Takes effect after a restart.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Service { name: "DiagTrack", private_start: 4, default_start: 2, default_delayed: false }],
    },
    Tweak {
        id: "dmwappush",
        category: Category::Telemetry,
        title: "Turn off WAP push message routing",
        summary: "Disables dmwappushservice, which routes device-management messages and telemetry.",
        tradeoff: "Work PCs managed by Intune/MDM may need it. Leave it on for company devices.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Service { name: "dmwappushservice", private_start: 4, default_start: 3, default_delayed: false }],
    },
    Tweak {
        id: "telemetry_tasks",
        category: Category::Telemetry,
        title: "Disable telemetry scheduled tasks",
        summary: "Turns off the compatibility appraiser, Customer Experience Improvement Program and feedback data collectors.",
        tradeoff: "Windows won't pre-check app compatibility before feature updates (Setup still checks during the update).",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Tasks(TELEMETRY_TASKS)],
    },
    Tweak {
        id: "ceip",
        category: Category::Telemetry,
        title: "Opt out of the Customer Experience Improvement Program",
        summary: "Sets the CEIP policy so Windows doesn't collect usage statistics for it.",
        tradeoff: "None for everyday use.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: M, path: r"SOFTWARE\Policies\Microsoft\SQMClient\Windows", name: "CEIPEnable", private: Dword(0), default: None }],
    },
    Tweak {
        id: "appcompat",
        category: Category::Telemetry,
        title: "Turn off application telemetry and inventory",
        summary: "Stops the Application Impact Telemetry agent and the inventory collector that report which programs you run.",
        tradeoff: "None for everyday use.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: M, path: r"SOFTWARE\Policies\Microsoft\Windows\AppCompat", name: "AITEnable", private: Dword(0), default: None },
            Reg { hive: M, path: r"SOFTWARE\Policies\Microsoft\Windows\AppCompat", name: "DisableInventory", private: Dword(1), default: None },
        ],
    },
    Tweak {
        id: "error_reporting",
        category: Category::Telemetry,
        title: "Turn off Windows Error Reporting",
        summary: "Crash and hang reports are no longer sent to Microsoft.",
        tradeoff: "Microsoft won't see crashes from your PC, so fixes for problems you hit may arrive later.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg {
            hive: M,
            path: r"SOFTWARE\Policies\Microsoft\Windows\Windows Error Reporting",
            name: "Disabled",
            private: Dword(1),
            default: None,
        }],
    },
    Tweak {
        id: "feedback",
        category: Category::Telemetry,
        title: "Never ask for feedback",
        summary: "Stops Windows from showing feedback surveys and notifications.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: U, path: r"Software\Microsoft\Siuf\Rules", name: "NumberOfSIUFInPeriod", private: Dword(0), default: None },
            Reg { hive: M, path: DATA_COLLECTION, name: "DoNotShowFeedbackNotifications", private: Dword(1), default: None },
        ],
    },
    Tweak {
        id: "tailored",
        category: Category::Telemetry,
        title: "Turn off tailored experiences",
        summary: "Stops Microsoft from using your diagnostic data to personalize tips, ads and recommendations.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg {
                hive: U,
                path: r"Software\Microsoft\Windows\CurrentVersion\Privacy",
                name: "TailoredExperiencesWithDiagnosticDataEnabled",
                private: Dword(0),
                default: Some(Dword(1)),
            },
            Reg { hive: U, path: CLOUD_CONTENT_USER, name: "DisableTailoredExperiencesWithDiagnosticData", private: Dword(1), default: None },
        ],
    },
    Tweak {
        id: "typing_data",
        category: Category::Telemetry,
        title: "Don't send inking and typing data",
        summary: "Stops sending samples of your handwriting and typing to Microsoft to improve recognition.",
        tradeoff: "None for everyday use.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: U, path: r"Software\Microsoft\Input\TIPC", name: "Enabled", private: Dword(0), default: Some(Dword(1)) }],
    },
    // ---------------------------------------------------------------- Ads & suggestions
    Tweak {
        id: "advertising_id",
        category: Category::Ads,
        title: "Turn off the advertising ID",
        summary: "Apps can no longer use a per-user advertising ID to track you across apps.",
        tradeoff: "Ads in apps become less personalized (not fewer).",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg {
            hive: U,
            path: r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo",
            name: "Enabled",
            private: Dword(0),
            default: Some(Dword(1)),
        }],
    },
    Tweak {
        id: "language_list",
        category: Category::Ads,
        title: "Don't share your language list with websites",
        summary: "Websites can't read your installed language list to tailor content.",
        tradeoff: "Some sites may pick a language from your browser settings instead.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: U, path: r"Control Panel\International\User Profile", name: "HttpAcceptLanguageOptOut", private: Dword(1), default: None }],
    },
    Tweak {
        id: "silent_apps",
        category: Category::Ads,
        title: "Stop automatic installation of suggested apps",
        summary: "Prevents Windows from silently installing promoted apps and games, and hides app suggestions in Start.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[cdm("SilentInstalledAppsEnabled"), cdm("SubscribedContent-338388Enabled"), cdm("SystemPaneSuggestionsEnabled")],
    },
    Tweak {
        id: "tips",
        category: Category::Ads,
        title: "Turn off tips and suggestions notifications",
        summary: "Stops \"Get tips, tricks and suggestions as you use Windows\" notifications.",
        tradeoff: "You won't see tips about new Windows features.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[cdm("SubscribedContent-338389Enabled"), cdm("SoftLandingEnabled")],
    },
    Tweak {
        id: "settings_suggestions",
        category: Category::Ads,
        title: "Hide suggested content in Settings",
        summary: "Removes promoted content and suggestions from the Settings app.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[cdm("SubscribedContent-338393Enabled"), cdm("SubscribedContent-353694Enabled"), cdm("SubscribedContent-353696Enabled")],
    },
    Tweak {
        id: "lockscreen_tips",
        category: Category::Ads,
        title: "No fun facts and tips on the lock screen",
        summary: "Removes promotional text and tips shown over lock screen pictures.",
        tradeoff: "Windows Spotlight pictures still work; only the overlay text goes.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[cdm("RotatingLockScreenOverlayEnabled"), cdm("SubscribedContent-338387Enabled")],
    },
    Tweak {
        id: "welcome",
        category: Category::Ads,
        title: "Skip the welcome experience after updates",
        summary: "Stops the full-screen \"Let's finish setting up\" pages that promote Microsoft services after updates.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            cdm("SubscribedContent-310093Enabled"),
            Reg {
                hive: U,
                path: r"Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement",
                name: "ScoobeSystemSettingEnabled",
                private: Dword(0),
                default: None,
            },
        ],
    },
    Tweak {
        id: "explorer_ads",
        category: Category::Ads,
        title: "Hide sync provider ads in File Explorer",
        summary: "Removes OneDrive / Microsoft 365 promotions shown in File Explorer.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: U, path: EXPLORER_ADV, name: "ShowSyncProviderNotifications", private: Dword(0), default: Some(Dword(1)) }],
    },
    Tweak {
        id: "consumer_features",
        category: Category::Ads,
        title: "Turn off Microsoft consumer experiences",
        summary: "Policy that blocks promoted third-party apps and suggestions for all users.",
        tradeoff: "None.",
        recommended: true,
        min_build: 0,
        note: Some("Fully enforced only on Enterprise and Education. Other editions honor it partly."),
        changes: &[Reg {
            hive: M,
            path: r"SOFTWARE\Policies\Microsoft\Windows\CloudContent",
            name: "DisableWindowsConsumerFeatures",
            private: Dword(1),
            default: None,
        }],
    },
    // ---------------------------------------------------------------- Search
    Tweak {
        id: "web_search",
        category: Category::Search,
        title: "Remove Bing web results from Start search",
        summary: "Start and taskbar search only show results from your PC, and your queries aren't sent to Bing.",
        tradeoff: "You can't search the web from Start. Takes effect after you sign out.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: U, path: r"Software\Policies\Microsoft\Windows\Explorer", name: "DisableSearchBoxSuggestions", private: Dword(1), default: None },
            Reg { hive: U, path: r"Software\Microsoft\Windows\CurrentVersion\Search", name: "BingSearchEnabled", private: Dword(0), default: None },
        ],
    },
    Tweak {
        id: "search_highlights",
        category: Category::Search,
        title: "Turn off search highlights",
        summary: "Removes trending searches and daily illustrations from the search box and search home.",
        tradeoff: "None.",
        recommended: true,
        min_build: 19044,
        note: None,
        changes: &[Reg { hive: U, path: SEARCH_SETTINGS, name: "IsDynamicSearchBoxEnabled", private: Dword(0), default: Some(Dword(1)) }],
    },
    Tweak {
        id: "cloud_search",
        category: Category::Search,
        title: "Don't search your cloud content",
        summary: "Windows search no longer includes OneDrive, Outlook and SharePoint content from your accounts.",
        tradeoff: "Cloud files and mail won't show up in Start search.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: U, path: SEARCH_SETTINGS, name: "IsMSACloudSearchEnabled", private: Dword(0), default: Some(Dword(1)) },
            Reg { hive: U, path: SEARCH_SETTINGS, name: "IsAADCloudSearchEnabled", private: Dword(0), default: Some(Dword(1)) },
        ],
    },
    Tweak {
        id: "cortana",
        category: Category::Search,
        title: "Turn off Cortana",
        summary: "Policy that disables Cortana for all users.",
        tradeoff: "No voice assistant (Cortana is retired on current builds anyway).",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: M, path: r"SOFTWARE\Policies\Microsoft\Windows\Windows Search", name: "AllowCortana", private: Dword(0), default: None }],
    },
    // ---------------------------------------------------------------- Activity
    Tweak {
        id: "activity_history",
        category: Category::Activity,
        title: "Turn off activity history",
        summary: "Windows stops recording which apps, files and sites you use, and doesn't upload that history.",
        tradeoff: "Timeline / \"pick up where you left off\" features stop working.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: M, path: SYSTEM_POLICY, name: "EnableActivityFeed", private: Dword(0), default: None },
            Reg { hive: M, path: SYSTEM_POLICY, name: "PublishUserActivities", private: Dword(0), default: None },
            Reg { hive: M, path: SYSTEM_POLICY, name: "UploadUserActivities", private: Dword(0), default: None },
        ],
    },
    Tweak {
        id: "clipboard_sync",
        category: Category::Activity,
        title: "Turn off clipboard sync across devices",
        summary: "Copied text and images are no longer uploaded to sync with your other devices.",
        tradeoff: "Local clipboard history (Win+V) still works; only cross-device sync stops.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: M, path: SYSTEM_POLICY, name: "AllowCrossDeviceClipboard", private: Dword(0), default: None }],
    },
    Tweak {
        id: "online_speech",
        category: Category::Activity,
        title: "Turn off online speech recognition",
        summary: "Your voice is no longer sent to Microsoft's cloud for recognition.",
        tradeoff: "Voice typing (Win+H) and dictation that rely on the cloud stop working.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[Reg {
            hive: U,
            path: r"Software\Microsoft\Speech_OneCore\Settings\OnlineSpeechPrivacy",
            name: "HasAccepted",
            private: Dword(0),
            default: None,
        }],
    },
    Tweak {
        id: "inking_personalization",
        category: Category::Activity,
        title: "Turn off inking & typing personalization",
        summary: "Windows stops building a personal dictionary from your typing, handwriting and contacts.",
        tradeoff: "Handwriting recognition and text suggestions learn less from you.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[
            Reg { hive: U, path: r"Software\Microsoft\InputPersonalization", name: "RestrictImplicitInkCollection", private: Dword(1), default: Some(Dword(0)) },
            Reg { hive: U, path: r"Software\Microsoft\InputPersonalization", name: "RestrictImplicitTextCollection", private: Dword(1), default: Some(Dword(0)) },
            Reg {
                hive: U,
                path: r"Software\Microsoft\InputPersonalization\TrainedDataStore",
                name: "HarvestContacts",
                private: Dword(0),
                default: Some(Dword(1)),
            },
            Reg { hive: U, path: r"Software\Microsoft\Personalization\Settings", name: "AcceptedPrivacyPolicy", private: Dword(0), default: Some(Dword(1)) },
        ],
    },
    Tweak {
        id: "app_launch_tracking",
        category: Category::Activity,
        title: "Don't track app launches",
        summary: "Windows stops recording which apps you open to rank Start and search results.",
        tradeoff: "Start's \"Most used\" list and search ranking stop adapting to you.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[Reg { hive: U, path: EXPLORER_ADV, name: "Start_TrackProgs", private: Dword(0), default: Some(Dword(1)) }],
    },
    // ---------------------------------------------------------------- AI
    Tweak {
        id: "recall",
        category: Category::Ai,
        title: "Turn off Recall snapshots",
        summary: "Prevents Windows from saving periodic screenshots of your activity for Recall.",
        tradeoff: "You can't search your past activity with Recall.",
        recommended: true,
        min_build: 26100,
        note: Some("Recall exists only on Copilot+ PCs running Windows 11 24H2 or later."),
        changes: &[Reg { hive: U, path: r"Software\Policies\Microsoft\Windows\WindowsAI", name: "DisableAIDataAnalysis", private: Dword(1), default: None }],
    },
    Tweak {
        id: "copilot",
        category: Category::Ai,
        title: "Turn off Windows Copilot",
        summary: "Removes the built-in Copilot sidebar and its taskbar button.",
        tradeoff: "No Copilot in Windows (the separate Copilot app is unaffected on newer builds).",
        recommended: false,
        min_build: 22621,
        note: None,
        changes: &[Reg {
            hive: U,
            path: r"Software\Policies\Microsoft\Windows\WindowsCopilot",
            name: "TurnOffWindowsCopilot",
            private: Dword(1),
            default: None,
        }],
    },
    // ---------------------------------------------------------------- App permissions
    Tweak {
        id: "perm_account",
        category: Category::Permissions,
        title: "Block apps from reading your account info",
        summary: "Store apps can't read your name, picture and other account details.",
        tradeoff: "Apps can't show your profile name/picture automatically.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\userAccountInformation")],
    },
    Tweak {
        id: "perm_diagnostics",
        category: Category::Permissions,
        title: "Block apps from reading other apps' diagnostics",
        summary: "Store apps can't see diagnostic information about the other apps you run.",
        tradeoff: "None for everyday use.",
        recommended: true,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\appDiagnostics")],
    },
    Tweak {
        id: "perm_location",
        category: Category::Permissions,
        title: "Block apps from using your location",
        summary: "Store apps can't ask for your location.",
        tradeoff: "Weather, Maps and Find My Device can't locate you automatically.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\location")],
    },
    Tweak {
        id: "perm_camera",
        category: Category::Permissions,
        title: "Block apps from using the camera",
        summary: "Store apps can't use your camera.",
        tradeoff: "Camera app, Teams (new) and other Store apps lose camera access.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\webcam")],
    },
    Tweak {
        id: "perm_microphone",
        category: Category::Permissions,
        title: "Block apps from using the microphone",
        summary: "Store apps can't use your microphone.",
        tradeoff: "Voice recorder, Teams (new) and other Store apps lose microphone access.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone")],
    },
    Tweak {
        id: "perm_notifications",
        category: Category::Permissions,
        title: "Block apps from reading your notifications",
        summary: "Store apps can't read the notifications other apps show you.",
        tradeoff: "Phone Link and smartwatch companion apps can't mirror notifications.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\userNotificationListener")],
    },
    Tweak {
        id: "perm_contacts",
        category: Category::Permissions,
        title: "Block apps from reading your contacts",
        summary: "Store apps can't read your People/contacts list.",
        tradeoff: "Mail, People and Phone Link can't look up contacts.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\contacts")],
    },
    Tweak {
        id: "perm_calendar",
        category: Category::Permissions,
        title: "Block apps from reading your calendar",
        summary: "Store apps can't read your calendar appointments.",
        tradeoff: "Calendar widgets and the taskbar agenda may stop showing events.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\appointments")],
    },
    Tweak {
        id: "perm_call_history",
        category: Category::Permissions,
        title: "Block apps from reading call history",
        summary: "Store apps can't read your phone call history.",
        tradeoff: "Phone Link can't show calls.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\phoneCallHistory")],
    },
    Tweak {
        id: "perm_email",
        category: Category::Permissions,
        title: "Block apps from reading your email",
        summary: "Store apps can't read or send email through your accounts.",
        tradeoff: "Store apps that share or send mail on your behalf stop working.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\email")],
    },
    Tweak {
        id: "perm_messages",
        category: Category::Permissions,
        title: "Block apps from reading messages",
        summary: "Store apps can't read or send text (SMS/MMS) messages.",
        tradeoff: "Phone Link can't show or send texts.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\chat")],
    },
    Tweak {
        id: "perm_radios",
        category: Category::Permissions,
        title: "Block apps from controlling radios",
        summary: "Store apps can't turn Bluetooth or Wi-Fi on and off.",
        tradeoff: "Apps that toggle Bluetooth for you (e.g. device setup apps) will ask you to do it.",
        recommended: false,
        min_build: 0,
        note: None,
        changes: &[consent(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\radios")],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_tweaks_have_changes() {
        let mut ids = std::collections::HashSet::new();
        for t in TWEAKS {
            assert!(ids.insert(t.id), "duplicate id {}", t.id);
            assert!(!t.changes.is_empty(), "{} has no changes", t.id);
        }
    }

    #[test]
    fn every_category_is_used() {
        for c in Category::ALL {
            assert!(TWEAKS.iter().any(|t| t.category == c), "{c:?} empty");
        }
    }
}
