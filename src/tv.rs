//! Android TV / projector by voice: switch on and off, open apps, search films
//! and YouTube, pause, volume. Talks to the TV over ADB (adb.rs); the TV needs
//! «Отладка по сети» (network ADB) on, and the first time a press on «Разрешить».

use std::{path::Path, time::Duration};

use crate::{adb, alice::TvAction};

/// Spoken app names → parts of their package names (first match wins).
/// (shown name, spoken names, package names or parts of them)
const APPS: &[(&str, &[&str], &[&str])] = &[
    ("YouTube", &["ютуб", "youtube", "ютьюб"], &["com.google.android.youtube.tv", "youtube.tv", "youtube"]),
    ("YouTube Music", &["ютуб музык", "ютуб мьюзик"], &["youtube.tv.unplugged", "youtube.music", "youtube.tvmusic"]),
    ("Кинопоиск", &["кинопоиск"], &["kinopoisk"]),
    ("ivi", &["иви", "ivi"], &["ivi"]),
    ("Okko", &["окко", "okko"], &["okko"]),
    ("Wink", &["винк", "wink"], &["wink", "ru.rt.video"]),
    ("KION", &["кион", "kion"], &["kion", "mts.mtstv", "ru.mts"]),
    ("Netflix", &["нетфликс", "netflix"], &["netflix"]),
    ("Twitch", &["твич", "twitch"], &["twitch"]),
    ("VK Видео", &["вк видео", "вк", "вконтакте"], &["vkvideo", "vk.video", "com.vk"]),
    ("RUTUBE", &["рутуб", "rutube"], &["rutube"]),
    ("PREMIER", &["премьер"], &["premier"]),
    ("Амедиатека", &["амедиатек"], &["amediateka"]),
    ("Смотрёшка", &["смотрешк"], &["smotreshka"]),
    ("Plex", &["плекс", "plex"], &["plex"]),
    ("Kodi", &["коди", "kodi"], &["kodi"]),
    ("VLC", &["влц", "vlc"], &["videolan"]),
    ("Spotify", &["спотифай", "spotify"], &["spotify"]),
    ("Яндекс Музыка", &["яндекс музык"], &["yandex.music"]),
    ("Браузер", &["браузер", "хром"], &["browser", "chrome"]),
    ("Настройки", &["настройки", "параметры"], &["com.android.tv.settings", "settings"]),
    ("Google Play", &["гугл плей", "плей маркет", "плеймаркет", "магазин"], &["com.android.vending"]),
    ("Lampa", &["лампа", "lampa"], &["lampa"]),
    ("KinoPub", &["кинопаб", "kinopub"], &["kinopub"]),
    ("HD VideoBox", &["видеобокс", "hd videobox"], &["videobox"]),
    ("SmartTube", &["смарт тьюб", "смарттьюб", "смарт туб", "смарт тюб", "смарт тьюбе", "smarttube", "smart tube"], &["liskovsoft.smarttubetv", "smarttube"]),
    ("Free TV", &["фри ти ви", "фритиви", "free tv", "free-tv", "atx", "атикс"], &["ru.cn.atx"]),
    ("LMD", &["лмд", "лнд", "лэнд", "делюкс", "lmd", "лм делюкс", "lm deluxe"], &["lazycatsoftware.lmd"]),
    ("Zona", &["зона", "зону", "зоне", "zona"], &["zona"]),
    ("VPN", &["впн", "vpn", "випиэн"], &["vpn", "wireguard", "amnezia", "outline", "hiddify", "v2ray", "nekobox", "openvpn"]),
];

/// Does what was said start with a known app's name («смарт тьюб», «ютуб»)?
pub fn app_named(said: &str) -> bool {
    let s = said.trim();
    APPS.iter().any(|(_, names, _)| names.iter().any(|n| s.starts_with(n)))
}

/// «Свои фразы для приложений»: (normalized alias, package, open/search/both),
/// kept in sync from the settings. The column's names come from `tv apps`.
static APP_PHRASES: std::sync::Mutex<Vec<(String, String, String)>> = std::sync::Mutex::new(Vec::new());

pub fn set_app_phrases(list: Vec<(String, String, String)>) {
    *APP_PHRASES.lock().unwrap() = list;
}

/// Does the phrase name one of the user's own TV apps? Used to tell a TV search
/// («найди фильм … на <алиасе>») from music («… на саундклауде»).
pub fn user_app_named(said: &str) -> bool {
    let s = crate::alice::normalize_phrase(said);
    APP_PHRASES.lock().unwrap().iter().any(|(alias, _, _)| *alias == s)
}

/// The user's app names: (alias, package, mode), for the voice parser.
pub fn app_phrases() -> Vec<(String, String, String)> {
    APP_PHRASES.lock().unwrap().clone()
}

/// Apps only the TV has: said without «на проекторе», the phrase is still the TV's.
pub fn tv_only_app(said: &str) -> bool {
    [
        "смарт тьюб", "смарттьюб", "смарт туб", "смарт тюб", "smarttube", "smart tube",
        "фри ти ви", "фритиви", "free tv", "free-tv", "atx", "атикс",
        "лмд", "lmd", "лм делюкс", "lm deluxe",
    ]
    .iter()
    .any(|n| said.contains(n))
}

// ---------------------------------------------------------------- the screen

/// Something on the TV's screen (uiautomator): its words and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct UiNode {
    pub label: String,
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub clickable: bool,
}

impl UiNode {
    fn center(&self) -> (i32, i32) {
        ((self.x1 + self.x2) / 2, (self.y1 + self.y2) / 2)
    }
    fn area(&self) -> i64 {
        i64::from(self.x2 - self.x1).max(0) * i64::from(self.y2 - self.y1).max(0)
    }
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// uiautomator's XML → the nodes that have words (text or content-desc) and a place.
pub fn ui_nodes(xml: &str) -> Vec<UiNode> {
    let mut out = Vec::new();
    for tag in xml.split("<node").skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        let text = attr(tag, "text").map(unescape).unwrap_or_default();
        let desc = attr(tag, "content-desc").map(unescape).unwrap_or_default();
        let label = if text.trim().is_empty() { desc } else { text };
        let Some(b) = attr(tag, "bounds") else { continue };
        let n: Vec<i32> = b.split(|c: char| !c.is_ascii_digit() && c != '-').filter(|x| !x.is_empty()).filter_map(|x| x.parse().ok()).collect();
        let [x1, y1, x2, y2] = n[..] else { continue };
        let yes = |k: &str| attr(tag, k) == Some("true");
        out.push(UiNode { label: label.trim().to_owned(), x1, y1, x2, y2, clickable: yes("clickable") || yes("focusable") });
    }
    out
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace('ё', "е").chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words said → the thing on the screen: the whole label first, then the label
/// inside what was said or the other way round, then the stems («историю» → «история»).
pub fn find_on_screen<'a>(nodes: &'a [UiNode], said: &str) -> Option<&'a UiNode> {
    let q = norm(said);
    if q.is_empty() {
        return None;
    }
    let stem = |w: &str| w.chars().take(5).collect::<String>();
    let score = |n: &UiNode| -> u8 {
        let l = norm(&n.label);
        if l.is_empty() {
            0
        } else if l == q {
            4
        } else if l.chars().count() >= 3 && (q.contains(&l) || l.contains(&q)) {
            3
        } else if q.split(' ').all(|w| l.split(' ').any(|lw| stem(lw) == stem(w))) {
            2
        } else if q.split(' ').next().is_some_and(|w| w.chars().count() >= 4 && l.split(' ').any(|lw| stem(lw) == stem(w))) {
            1
        } else {
            0
        }
    };
    nodes
        .iter()
        .filter(|n| n.area() > 0)
        .map(|n| (score(n), n))
        .filter(|(s, _)| *s > 0)
        // Best match; then a pressable one; then the smaller (the item, not its whole row).
        .max_by_key(|(s, n)| (*s, n.clickable, -n.area()))
        .map(|(_, n)| n)
}

/// The first video of a list or grid: pressable, with words, out of the left menu
/// (the left fifth of the screen), not a menu item; the top one, then the left one.
pub fn first_item<'a>(nodes: &'a [UiNode]) -> Option<&'a UiNode> {
    let w = nodes.iter().map(|n| n.x2).max().unwrap_or(0);
    let h = nodes.iter().map(|n| n.y2).max().unwrap_or(0);
    let menu = ["главная", "история", "подписки", "поиск", "настройки", "библиотека", "плейлисты", "home", "history", "subscriptions", "search", "settings"];
    nodes
        .iter()
        .filter(|n| n.clickable && !n.label.is_empty() && n.x1 >= w / 5 && n.y1 > h / 10)
        .filter(|n| !menu.contains(&norm(&n.label).as_str()))
        .filter(|n| n.area() > i64::from(w) * i64::from(h) / 400)
        .min_by_key(|n| (n.y1 / 40, n.x1))
}

/// The words a VPN's main button may have, best first.
const START_WORDS: [&str; 8] = ["подключить", "подключиться", "включить", "connect", "start", "старт", "запустить", "tap to connect"];

fn dump_screen(sh: &dyn Fn(&str) -> Result<String, String>) -> Result<Vec<UiNode>, String> {
    // uiautomator needs a still screen; a moving one («could not get idle state») is tried again.
    for _ in 0..3 {
        let out = sh("uiautomator dump /data/local/tmp/soundya-ui.xml >/dev/null 2>&1; cat /data/local/tmp/soundya-ui.xml 2>/dev/null")?;
        let nodes = ui_nodes(&out);
        if !nodes.is_empty() {
            return Ok(nodes);
        }
        std::thread::sleep(Duration::from_millis(1200));
    }
    Err("Не получилось прочитать экран телевизора".into())
}

fn tap(sh: &dyn Fn(&str) -> Result<String, String>, n: &UiNode) -> Result<(), String> {
    let (x, y) = n.center();
    sh(&format!("input tap {x} {y}")).map(|_| ())
}

fn open_app(sh: &dyn Fn(&str) -> Result<String, String>, name: &str) -> Result<String, String> {
    let list = sh("pm list packages")?;
    let packages: Vec<String> = list.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(str::to_owned).collect();
    let Some(pkg) = find_package(name, &packages) else {
        return Err(format!("Не нашла на телевизоре приложение {name}"));
    };
    let out = sh(&launch_script(&pkg))?;
    if out.contains("Error") || out.contains("No activities") || out.contains("Exception") {
        return Err(format!("Телевизор не открыл {name}"));
    }
    Ok(pkg)
}

/// Labels on the screen for the log when nothing matched (to tune the words).
fn seen(nodes: &[UiNode]) -> String {
    nodes.iter().filter(|n| !n.label.is_empty()).take(25).map(|n| n.label.chars().take(30).collect::<String>()).collect::<Vec<_>>().join(" | ")
}

fn translit(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let t = match c {
            'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' | 'ё' | 'э' => "e", 'ж' => "zh",
            'з' => "z", 'и' | 'й' | 'ы' => "i", 'к' => "k", 'л' => "l", 'м' => "m", 'н' => "n", 'о' => "o",
            'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u", 'ф' => "f", 'х' => "h", 'ц' => "c",
            'ч' => "ch", 'ш' => "sh", 'щ' => "sch", 'ю' => "yu", 'я' => "ya",
            c if c.is_ascii_alphanumeric() => {
                out.push(c.to_ascii_lowercase());
                continue;
            }
            _ => "",
        };
        out.push_str(t);
    }
    out
}

/// A package as people know it: «com.google.android.youtube.tv» → «YouTube».
/// A known app by its package (the longest matching part wins: «…youtube.tvmusic»
/// is YouTube Music); another: the telling part of its name, the package after it.
pub fn label(package: &str) -> String {
    let full = |p: &str| p.matches('.').count() >= 2;
    let known = APPS
        .iter()
        .filter_map(|(shown, _, parts)| {
            let len = parts.iter().filter(|p| if full(p) { package == **p } else { package.contains(**p) }).map(|p| p.len()).max()?;
            Some((len, *shown))
        })
        .max_by_key(|(len, _)| *len);
    if let Some((_, shown)) = known {
        return shown.to_owned();
    }
    const PLAIN: [&str; 14] = ["com", "org", "net", "ru", "tv", "app", "apps", "android", "google", "mobile", "atv", "androidtv", "release", "free"];
    let word = package.split('.').filter(|w| !PLAIN.contains(w) && w.len() > 1).max_by_key(|w| w.len()).unwrap_or(package);
    let mut c = word.chars();
    let word = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    format!("{word} ({package})")
}

/// The installed package for a spoken app name.
pub fn find_package(spoken: &str, packages: &[String]) -> Option<String> {
    let spoken = spoken.trim().to_lowercase();
    // The most specific name wins: «ютуб музыку» is YouTube Music, not YouTube.
    let entry = APPS
        .iter()
        .filter_map(|(_, names, parts)| {
            let len = names.iter().filter(|n| spoken.starts_with(**n)).map(|n| n.len()).max()?;
            Some((len, *parts))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, parts)| parts);
    if let Some(parts) = entry {
        // Parts of other apps: «…youtube.tvmusic» contains «…youtube.tv» but is YouTube Music.
        // A full package name («com.google.android.youtube.tv») only matches itself.
        let full = |p: &str| p.matches('.').count() >= 2;
        let others: Vec<&str> =
            APPS.iter().flat_map(|(_, _, p)| p.iter().copied()).filter(|p| !parts.contains(p) && !full(p)).collect();
        for part in parts {
            if let Some(p) = packages.iter().find(|p| p.as_str() == *part) {
                return Some(p.clone());
            }
            if full(part) {
                continue;
            }
            let found = packages.iter().find(|p| {
                // …and not a package that another app's name explains better.
                p.contains(part) && !others.iter().any(|o| p.contains(o) && !part.contains(o))
            });
            if let Some(p) = found {
                return Some(p.clone());
            }
        }
        // A known app that is not installed: not some other app with a similar name.
        return None;
    }
    // Anything else: the name, in Latin letters, inside the package name.
    // Within one part of the package name: «com.amazon.avod» joined up would
    // contain «zona».
    for t in spellings(&spoken) {
        if let Some(p) = packages.iter().find(|p| p.split(['.', '_', '-']).any(|part| part.contains(&t))) {
            return Some(p.clone());
        }
    }
    None
}

/// How a spoken app name may appear in a package name, best first: as said
/// («zona»), back from the case Alice heard («зону» → zona), with
/// other spellings of the sounds (кс → x, ю → iu/u, й → y…), and the stem.
fn spellings(spoken: &str) -> Vec<String> {
    let words: String = spoken.split_whitespace().collect::<Vec<_>>().join("");
    let mut bases = vec![words.clone()];
    // «Открой зону / кинопоиск / тиви»: accusative and similar endings back to the name.
    for (end, base) in [("у", "а"), ("ю", "я"), ("ом", ""), ("е", "а")] {
        if let Some(stem) = words.strip_suffix(end) {
            bases.push(format!("{stem}{base}"));
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: String| {
        if s.len() >= 3 && !out.contains(&s) {
            out.push(s);
        }
    };
    for b in &bases {
        let t = translit(b);
        add(t.clone());
        add(t.replace("ks", "x").replace("yu", "u").replace("ya", "ia").replace("ts", "z").replace("kh", "h"));
        add(t.replace("yu", "iu").replace("zh", "j").replace('c', "ts"));
        add(t.replace("i", "y"));
    }
    // The stem without its last vowel («zon» for «зона»), only when long enough
    // not to match everything.
    let t = translit(&words);
    let stem = t.trim_end_matches(['a', 'e', 'i', 'o', 'u', 'y']);
    if stem.len() >= 4 {
        add(stem.to_owned());
    }
    out
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else if b == b' ' { "+".into() } else { format!("%{b:02X}") })
        .collect()
}

fn speech(e: &adb::Error) -> String {
    match e {
        adb::Error::Unreachable(_) => {
            "Телевизор не отвечает. Он включён, в той же сети и с включённой отладкой по сети?".into()
        }
        adb::Error::NeedsConfirm => {
            "На экране телевизора появился запрос на отладку. Нажмите «Разрешить» и повторите".into()
        }
        adb::Error::Other(e) => format!("Телевизор не выполнил команду: {e}"),
    }
}

/// Runs the action; the answer is a short sentence for Alice to say.
/// The ADB key, read from disk once (it was read and parsed on every key press).
fn key(dir: &Path) -> Result<std::sync::Arc<adb::Key>, String> {
    static KEY: std::sync::Mutex<Option<(std::path::PathBuf, std::sync::Arc<adb::Key>)>> = std::sync::Mutex::new(None);
    let mut g = KEY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((d, k)) = &*g {
        if d == dir {
            return Ok(k.clone());
        }
    }
    let k = std::sync::Arc::new(adb::Key::load_or_create(dir).map_err(|e| format!("нет ключа для телевизора: {e}"))?);
    *g = Some((dir.to_owned(), k.clone()));
    Ok(k)
}

/// Wakes the TV and dismisses its screensaver; a sleeping TV needs a moment.
fn wake(sh: &dyn Fn(&str) -> Result<String, String>) {
    let asleep = sh("dumpsys power | grep -m1 mWakefulness=").map(|o| !o.contains("Awake")).unwrap_or(false);
    let _ = sh("input keyevent KEYCODE_WAKEUP");
    if asleep {
        std::thread::sleep(Duration::from_millis(1500));
    }
}

pub fn run(host: &str, dir: &Path, action: &TvAction) -> Result<String, String> {
    let key = key(dir)?;
    // Waiting for «Разрешить» makes sense only for the check from the settings page.
    let wait = if matches!(action, TvAction::Ping) { Duration::from_secs(40) } else { Duration::from_secs(8) };
    // One connection kept between presses: the remote answers without a new handshake.
    let sh = |cmd: &str| adb::shell_kept(host, &key, cmd, wait).map_err(|e| speech(&e));
    match action {
        TvAction::Ping => {
            let out = sh("getprop ro.product.manufacturer; getprop ro.product.model")?;
            let name = out.split_whitespace().collect::<Vec<_>>().join(" ");
            Ok(format!("Телевизор на связи: {}", if name.is_empty() { "Android" } else { &name }))
        }
        TvAction::Wake => sh("input keyevent KEYCODE_WAKEUP").map(|_| "Включаю".into()),
        TvAction::Sleep => sh("input keyevent KEYCODE_SLEEP").map(|_| "Выключаю".into()),
        TvAction::Key(code, n) => {
            let keys = vec![*code; usize::from(*n)].join(" ");
            sh(&format!("input keyevent {keys}")).map(|_| "Готово".into())
        }
        TvAction::Search(q, app) => {
            // «Алиса, найди фильм Колобок»: the TV on first, then the search (#SY-F351993A).
            wake(&sh);
            // No app named — the TV's own (Google) search, as asked.
            let Some(spoken) = app else {
                return sh(&global_search(q)).map(|_| format!("Ищу {q} в общем поиске"));
            };
            // A user phrase stores the exact package: no guessing by its name.
            let pkg = if valid_package(spoken) {
                spoken.clone()
            } else {
                let list = sh("pm list packages")?;
                let packages: Vec<String> =
                    list.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(str::to_owned).collect();
                find_package(spoken, &packages).ok_or_else(|| format!("Не нашла на телевизоре приложение {spoken}"))?
            };
            match search_in_app(&sh, &pkg, q) {
                Ok(text) => Ok(text),
                Err(e) => {
                    eprintln!("tv: search in {pkg}: {e}; the TV's own search");
                    sh(&global_search(q)).map(|_| format!("Не получилось найти в {} — ищу в общем поиске", label(&pkg)))
                }
            }
        }
        TvAction::YouTube(q) => {
            let url = format!("https://www.youtube.com/results?search_query={}", percent_encode(q));
            let out = sh(&format!("am start -a android.intent.action.VIEW -d {}", adb::quote(&url)))?;
            if out.contains("Error") {
                // No app takes the link: the TV's own search, which includes YouTube.
                sh(&format!("am start -a android.search.action.GLOBAL_SEARCH --es query {}", adb::quote(q)))?;
            }
            Ok(format!("Ищу на ютубе {q}"))
        }
        TvAction::Apps => {
            // Apps with a TV launcher entry (what the TV's home screen shows);
            // older TVs without `query-activities`: third-party packages.
            let out = sh("cmd package query-activities --brief -a android.intent.action.MAIN -c android.intent.category.LEANBACK_LAUNCHER 2>/dev/null")?;
            let mut apps = packages_of(&out);
            if apps.is_empty() {
                let list = sh("pm list packages -3")?;
                apps = list.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(str::to_owned).collect();
            }
            apps.sort();
            apps.dedup();
            let names: serde_json::Map<String, serde_json::Value> = apps.iter().map(|a| (a.clone(), label(a).into())).collect();
            Ok(serde_json::json!({ "apps": apps, "names": names }).to_string())
        }
        TvAction::OpenPackage(pkg) => {
            if !valid_package(pkg) {
                return Err("неверное имя пакета".into());
            }
            wake(&sh);
            let out = sh(&launch_script(pkg))?;
            if out.contains("Error") || out.contains("No activities") || out.contains("Exception") {
                return Err(format!("Телевизор не открыл {pkg}"));
            }
            Ok("Открываю".into())
        }
        TvAction::Close(name) => {
            let pkg = if valid_package(name) && name.contains('.') {
                name.clone()
            } else {
                let list = sh("pm list packages")?;
                let packages: Vec<String> =
                    list.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(str::to_owned).collect();
                find_package(name, &packages).ok_or_else(|| format!("Не нашла на телевизоре приложение {name}"))?
            };
            sh(&format!("am force-stop {pkg}")).map(|_| format!("Закрыла {name}"))
        }
        // Handled by the controller (no TV involved).
        TvAction::RemoteMode(_) => Ok(String::new()),
        TvAction::Tap(what) => {
            let nodes = dump_screen(&sh)?;
            match find_on_screen(&nodes, what) {
                Some(n) => {
                    tap(&sh, n)?;
                    Ok(format!("Нажала {}", n.label))
                }
                None => {
                    eprintln!("tv: «{what}» not on the screen; seen: {}", seen(&nodes));
                    Err(format!("Не вижу на экране «{what}»"))
                }
            }
        }
        TvAction::OpenIn(app, section) => {
            wake(&sh);
            open_app(&sh, app)?;
            // The app's first screen takes a few seconds.
            for attempt in 0..4 {
                std::thread::sleep(Duration::from_millis(if attempt == 0 { 3500 } else { 1500 }));
                let Ok(nodes) = dump_screen(&sh) else { continue };
                if let Some(n) = find_on_screen(&nodes, section) {
                    tap(&sh, n)?;
                    return Ok(format!("Открываю {section}"));
                }
                if attempt == 3 {
                    eprintln!("tv: section «{section}» not found in {app}; seen: {}", seen(&nodes));
                }
            }
            Err(format!("Открыла {app}, но не нашла там «{section}»"))
        }
        TvAction::PlayFirst => {
            let nodes = dump_screen(&sh)?;
            match first_item(&nodes) {
                Some(n) => {
                    tap(&sh, n)?;
                    Ok(format!("Включаю {}", n.label.chars().take(60).collect::<String>()))
                }
                None => {
                    // Nothing recognisable: the focused item, as the remote's OK would.
                    eprintln!("tv: no first item; seen: {}", seen(&nodes));
                    sh("input keyevent KEYCODE_DPAD_CENTER").map(|_| "Включаю".into())
                }
            }
        }
        TvAction::OpenAndStart(app) => {
            wake(&sh);
            open_app(&sh, app)?;
            std::thread::sleep(Duration::from_millis(3500));
            let nodes = dump_screen(&sh).unwrap_or_default();
            if let Some(n) = START_WORDS.iter().find_map(|w| find_on_screen(&nodes, w)) {
                tap(&sh, n)?;
                return Ok(format!("Открыла {app} и нажала {}", n.label));
            }
            // Most TV VPN apps open with the big button in focus.
            eprintln!("tv: no start button in {app}; seen: {}", seen(&nodes));
            sh("input keyevent KEYCODE_DPAD_CENTER").map(|_| format!("Открыла {app} и нажала главную кнопку"))
        }
        TvAction::Open(name) => {
            wake(&sh);
            let list = sh("pm list packages")?;
            let packages: Vec<String> =
                list.lines().filter_map(|l| l.trim().strip_prefix("package:")).map(str::to_owned).collect();
            let Some(pkg) = find_package(name, &packages) else {
                eprintln!("tv: no app for {name:?} among {} packages", packages.len());
                return Err(format!("Не нашла на телевизоре приложение {name}"));
            };
            let out = sh(&launch_script(&pkg))?;
            eprintln!("tv: open {name:?} -> {pkg}: {}", out.trim().replace('\n', " | "));
            if out.contains("Error") || out.contains("No activities") || out.contains("Exception") {
                return Err(format!("Телевизор не открыл {name}"));
            }
            Ok(format!("Открываю {name}"))
        }
    }
}

/// «com.google.android.youtube.tv/…» lines → packages.
fn packages_of(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.trim().split_once('/').map(|(p, _)| p.trim().to_owned()))
        .filter(|p| valid_package(p))
        .collect()
}

/// «… на кинопоиске» → («…», «кинопоиске»): the app named at the end. A TV
/// (телевизор, проектор…) is not an app. The rest of the phrase is the query.
pub fn split_app_suffix(text: &str) -> Option<(String, String)> {
    const TV: [&str; 14] = [
        "телевизор", "телевизора", "телевизоре", "телевизору", "телек", "телеке", "телик", "телике", "тв", "тиви",
        "проектор", "проектора", "проекторе", "приставке",
    ];
    let t = text.trim();
    let at = t.rfind(" на ")?;
    let (head, tail) = t.split_at(at);
    let app = tail.trim_start().strip_prefix("на ")?.trim();
    if app.is_empty() || app.split_whitespace().count() > 3 || TV.contains(&app.split_whitespace().next().unwrap_or("")) {
        return None;
    }
    let head = head.trim();
    (!head.is_empty()).then(|| (head.to_owned(), app.to_owned()))
}

/// The search inside the app named by the user: Kinopoisk's and YouTube's own
/// screens, Zona's film card (its public search API); another app — the SEARCH
/// intent. An error means «no search here»: the caller opens the TV's own search.
fn search_in_app(sh: &dyn Fn(&str) -> Result<String, String>, pkg: &str, q: &str) -> Result<String, String> {
    // LMD's search screen is its own activity: the query is the «query» extra.
    // It is not exported to other apps, but the TV's shell may start it.
    if pkg == "com.lazycatsoftware.lmd" {
        let activity = "com.lazycatsoftware.lmd/com.lazycatsoftware.lazymediadeluxe.ui.tv.activities.ActivityTvSearch";
        let out = sh(&format!("am start -n {activity} --es query {}", adb::quote(q)))?;
        if start_failed(&out) {
            return Err(out.trim().to_owned());
        }
        return Ok(format!("Ищу {q} в LMD"));
    }
    if pkg == "mobi.zona" {
        let (id, name) = zona_search(q)?;
        start_view(sh, pkg, &zona_program_uri(id))?;
        return Ok(format!("Нашла «{name}» в Zona"));
    }
    if let Some(uri) = search_deeplink(pkg, q) {
        start_view(sh, pkg, &uri)?;
        return Ok(format!("Ищу {q} в {}", label(pkg)));
    }
    // The SEARCH intent: Android TV apps rarely declare it for other apps, so
    // «am start» usually finds no activity — that is the caller's fallback.
    let cmd = format!("am start -a android.intent.action.SEARCH -p {pkg} --es query {}", adb::quote(q));
    match sh(&cmd) {
        Ok(out) if !start_failed(&out) => Ok(format!("Ищу {q} в {}", label(pkg))),
        Ok(out) => Err(format!("{pkg} does not search ({})", out.trim())),
        Err(e) => Err(e),
    }
}

/// «am start» did not start anything: no activity, no permission, an exception.
fn start_failed(out: &str) -> bool {
    ["Error", "Exception", "Permission Denial"].iter().any(|e| out.contains(e))
}

/// Opens an address exactly in the app (`-p`: not another app that also takes it).
fn start_view(sh: &dyn Fn(&str) -> Result<String, String>, pkg: &str, uri: &str) -> Result<(), String> {
    let out = sh(&format!("am start -a android.intent.action.VIEW -d {} -p {pkg}", adb::quote(uri)))?;
    if start_failed(&out) {
        Err(out.trim().to_owned())
    } else {
        Ok(())
    }
}

/// The TV app's film card, as its own deeplink handler expects it.
fn zona_program_uri(id: i64) -> String {
    format!("http://mobi.zona/program/{id}")
}

/// Zona's public search (no login): the first film/series and its id; the TV app
/// opens `http://mobi.zona/program/<id>`. Two API hosts, as the app does.
fn zona_search(q: &str) -> Result<(i64, String), String> {
    let query = percent_encode_query(q);
    let mut last = String::new();
    for host in ["apir1.mzona.net", "apir0.mzona.net"] {
        let url = format!("https://{host}/search?app_type=zona&query={query}&limit=10&offset=0");
        let resp = match ureq::get(&url).timeout(Duration::from_secs(10)).call() {
            Ok(r) => r,
            Err(e) => {
                last = format!("{host}: {e}");
                continue;
            }
        };
        let json: serde_json::Value = match resp.into_json() {
            Ok(v) => v,
            Err(e) => {
                last = format!("{host}: {e}");
                continue;
            }
        };
        let Some(first) = json.get("data").and_then(|v| v.as_array()).and_then(|a| a.first()) else {
            return Err("Zona: ничего не найдено".into());
        };
        let Some(id) = first.get("id").and_then(|v| v.as_i64()) else {
            return Err("Zona: в ответе нет id".into());
        };
        let name = first.get("name").and_then(|v| v.as_str()).unwrap_or(q).to_owned();
        return Ok((id, name));
    }
    Err(format!("Zona API: {last}"))
}

/// Apps whose search screen has an address of its own (checked on real TVs):
/// Kinopoisk's `kpatv://home?page=search&search_query=…` and YouTube (also
/// YouTube-based SmartTube) — the site's results page. None: the SEARCH intent.
fn search_deeplink(app: &str, q: &str) -> Option<String> {
    let q = percent_encode_query(q);
    let p = app.to_ascii_lowercase();
    if p == "ru.kinopoisk.tv" {
        return Some(format!("kpatv://home?page=search&search_query={q}"));
    }
    // YouTube Music has its own package names and its search is not the video one.
    if p.contains("youtube.tvmusic") || p.contains("youtube.music") {
        return None;
    }
    if p.contains("youtube") || p.contains("smarttube") {
        return Some(format!("https://www.youtube.com/results?search_query={q}"));
    }
    None
}

/// Text for a URI query: everything but the unreserved characters is
/// percent-encoded (`пробел` → `%20`, not `+`: Uri.getQueryParameter does not
/// turn `+` back into a space on every Android version).
fn percent_encode_query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn global_search(q: &str) -> String {
    format!("am start -a android.search.action.GLOBAL_SEARCH --es query {}", adb::quote(q))
}

/// A package name safe to put into a shell command.
pub fn valid_package(p: &str) -> bool {
    !p.is_empty() && p.len() < 200 && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// Starts an app's main screen: the TV launcher entry (Android TV apps), else the
/// phone launcher entry, else whatever monkey finds.
fn launch_script(pkg: &str) -> String {
    let resolve = |cat: &str| {
        format!("cmd package resolve-activity --brief -a android.intent.action.MAIN -c android.intent.category.{cat} {pkg} 2>/dev/null | tail -n 1")
    };
    format!(
        "c=$({}); case \"$c\" in */*) ;; *) c=$({});; esac; \
         case \"$c\" in */*) am start -n \"$c\" 2>&1 ;; *) monkey -p {pkg} 1 2>&1 ;; esac",
        resolve("LEANBACK_LAUNCHER"),
        resolve("LAUNCHER")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // A SmartTube-like screen: the menu on the left, a grid of videos.
    const SCREEN: &str = r#"<?xml version='1.0'?><hierarchy rotation="0">
<node index="0" text="" class="android.widget.FrameLayout" content-desc="" clickable="false" focusable="false" bounds="[0,0][1920,1080]">
<node text="Главная" content-desc="" clickable="true" focusable="true" bounds="[20,200][300,260]" />
<node text="История" content-desc="" clickable="true" focusable="true" bounds="[20,280][300,340]" />
<node text="" content-desc="Поиск" clickable="true" focusable="true" bounds="[20,120][300,180]" />
<node text="Как приготовить плов &amp; не сжечь" content-desc="" clickable="true" focusable="true" bounds="[420,200][900,520]" />
<node text="Обзор проектора" content-desc="" clickable="true" focusable="true" bounds="[940,200][1420,520]" />
<node text="Вторая строка" content-desc="" clickable="true" focusable="true" bounds="[420,560][900,880]" />
</node></hierarchy>"#;

    #[test]
    fn reads_the_screen_and_finds_what_was_said() {
        let nodes = ui_nodes(SCREEN);
        assert_eq!(nodes.len(), 7);
        assert_eq!(nodes[4].label, "Как приготовить плов & не сжечь");
        assert_eq!(nodes[3].label, "Поиск", "content-desc when there is no text");
        assert_eq!(find_on_screen(&nodes, "историю").unwrap().label, "История", "the case Alice heard");
        assert_eq!(find_on_screen(&nodes, "история просмотров").unwrap().label, "История");
        assert_eq!(find_on_screen(&nodes, "поиск").unwrap().center(), (160, 150));
        assert_eq!(find_on_screen(&nodes, "обзор").unwrap().label, "Обзор проектора");
        assert!(find_on_screen(&nodes, "настройки").is_none());
        assert_eq!(first_item(&nodes).unwrap().label, "Как приготовить плов & не сжечь", "top left of the grid, not the menu");
    }

    #[test]
    fn smarttube_and_vpn_by_name() {
        let pk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let installed = pk(&["com.google.android.youtube.tv", "com.liskovsoft.smarttubetv.beta", "org.amnezia.vpn"]);
        assert_eq!(find_package("смарт тьюб", &installed).as_deref(), Some("com.liskovsoft.smarttubetv.beta"));
        assert_eq!(find_package("впн", &installed).as_deref(), Some("org.amnezia.vpn"));
        assert!(START_WORDS.iter().any(|w| find_on_screen(&ui_nodes(r#"<node text="Подключиться" clickable="true" bounds="[800,500][1100,600]" />"#), w).is_some()));
    }

    #[test]
    fn packages_get_names_people_know() {
        // The scenario editor listed bare package names; nobody could tell what opens.
        assert_eq!(label("com.google.android.youtube.tv"), "YouTube");
        assert_eq!(label("com.google.android.youtube.tvmusic"), "YouTube Music");
        assert_eq!(label("ru.kinopoisk.tv"), "Кинопоиск");
        assert_eq!(label("mobi.zona"), "Zona");
        assert_eq!(label("ru.cn.atx"), "Free TV");
        assert_eq!(label("com.lazycatsoftware.lmd"), "LMD");
        assert_eq!(label("com.android.vending"), "Google Play");
        assert_eq!(label("com.example.wink.tv"), "Wink");
        // Unknown: the telling part of the name, the package after it.
        assert_eq!(label("com.teamsmart.videomanager.tv"), "Videomanager (com.teamsmart.videomanager.tv)");
        assert_eq!(label("org.example.app"), "Example (org.example.app)");
    }

    #[test]
    fn finds_apps_by_spoken_name() {
        let pk: Vec<String> = [
            "com.google.android.youtube.tv",
            "ru.kinopoisk.tv",
            "ru.ivi.client",
            "com.netflix.ninja",
            "org.videolan.vlc",
            "com.android.tv.settings",
            "tv.okko.androidtv",
            "mobi.zona",
            "ru.cn.atx",
            "com.lazycatsoftware.lmd",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(find_package("ютуб", &pk).as_deref(), Some("com.google.android.youtube.tv"));
        assert_eq!(find_package("зоне", &pk).as_deref(), Some("mobi.zona"));
        assert_eq!(find_package("зону", &pk).as_deref(), Some("mobi.zona"));
        assert_eq!(find_package("фри ти ви", &pk).as_deref(), Some("ru.cn.atx"));
        assert_eq!(find_package("free tv", &pk).as_deref(), Some("ru.cn.atx"));
        assert_eq!(find_package("лмд", &pk).as_deref(), Some("com.lazycatsoftware.lmd"));
        assert_eq!(find_package("кинопоиск", &pk).as_deref(), Some("ru.kinopoisk.tv"));
        assert_eq!(find_package("иви", &pk).as_deref(), Some("ru.ivi.client"));
        assert_eq!(find_package("нетфликс", &pk).as_deref(), Some("com.netflix.ninja"));
        assert_eq!(find_package("влц", &pk).as_deref(), Some("org.videolan.vlc"));
        assert_eq!(find_package("настройки", &pk).as_deref(), Some("com.android.tv.settings"));
        assert_eq!(find_package("окко", &pk).as_deref(), Some("tv.okko.androidtv"));
        assert_eq!(find_package("твич", &pk), None);
        // Xiaomi L1 Pro lists YouTube Music before YouTube.
        let pk: Vec<String> =
            ["com.google.android.youtube.tvmusic", "com.google.android.youtube.tv"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(find_package("youtube", &pk).as_deref(), Some("com.google.android.youtube.tv"));
        assert_eq!(find_package("ютуб", &pk).as_deref(), Some("com.google.android.youtube.tv"));
        assert_eq!(find_package("ютуб музыку", &pk).as_deref(), Some("com.google.android.youtube.tvmusic"));
        let only_music: Vec<String> = vec!["com.google.android.youtube.tvmusic".into()];
        assert_eq!(find_package("ютуб", &only_music), None);
    }

    #[test]
    fn app_suffix_is_taken_from_the_phrase() {
        assert_eq!(
            split_app_suffix("найди фильм колобок на кинопоиске"),
            Some(("найди фильм колобок".into(), "кинопоиске".into()))
        );
        assert_eq!(
            split_app_suffix("включи фильм колобок на зоне"),
            Some(("включи фильм колобок".into(), "зоне".into()))
        );
        assert_eq!(
            split_app_suffix("найди фильм колобок на смарт тьюбе"),
            Some(("найди фильм колобок".into(), "смарт тьюбе".into()))
        );
        assert_eq!(split_app_suffix("найди фильм колобок"), None, "no app");
        assert_eq!(split_app_suffix("найди фильм колобок на телевизоре"), None, "the TV is not an app");
        assert_eq!(split_app_suffix("включи колобка на зоне"), Some(("включи колобка".into(), "зоне".into())));
    }

    #[test]
    fn searches_in_the_named_app() {
        // Kinopoisk's own search screen: the SEARCH intent is not declared for other
        // apps (no CATEGORY_DEFAULT), its deeplink is — checked on a real TV.
        let kp = search_deeplink("ru.kinopoisk.tv", "колобок 2").unwrap();
        assert_eq!(kp, "kpatv://home?page=search&search_query=%D0%BA%D0%BE%D0%BB%D0%BE%D0%B1%D0%BE%D0%BA%202");
        // YouTube and SmartTube open their results by the site's address.
        for app in ["com.google.android.youtube.tv", "org.smarttube.stable", "com.liskovsoft.smarttubetv.beta"] {
            let yt = search_deeplink(app, "котики").unwrap();
            assert!(yt.starts_with("https://www.youtube.com/results?search_query=%D0%BA%D0%BE%D1%82%D0%B8%D0%BA%D0%B8"), "{yt}");
        }
        // YouTube Music is not the video search.
        assert!(search_deeplink("com.google.android.youtube.tvmusic", "котики").is_none());
        // Zona: its film card (the API itself is checked on a real TV).
        assert_eq!(zona_program_uri(843042), "http://mobi.zona/program/843042");
        // Only a package name goes into the shell; the commands are valid shell.
        let view = format!("am start -a android.intent.action.VIEW -d {} -p ru.kinopoisk.tv", adb::quote(&kp));
        let search = format!("am start -a android.intent.action.SEARCH -p tv.okko.androidtv --es query {}", adb::quote("колобок"));
        for s in [view, search, global_search("it's")] {
            let ok = std::process::Command::new("sh").arg("-n").arg("-c").arg(&s).status().unwrap().success();
            assert!(ok, "{s}");
        }
        assert!(!valid_package("x; reboot"), "only a package name goes into the shell");
    }

    #[test]
    fn lmd_search_opens_its_search_screen() {
        let cmds = std::cell::RefCell::new(Vec::new());
        let sh = |c: &str| {
            cmds.borrow_mut().push(c.to_owned());
            Ok(String::new())
        };
        let text = search_in_app(&sh, "com.lazycatsoftware.lmd", "колобок").unwrap();
        assert!(text.contains("LMD"), "{text}");
        let cmds = cmds.borrow();
        assert!(cmds[0].contains("ActivityTvSearch"), "{}", cmds[0]);
        assert!(cmds[0].contains("--es query 'колобок'"), "{}", cmds[0]);
        // «Включи фри ти ви» and «включи лмд» are the TV's even without its name.
        assert!(tv_only_app("включи фри ти ви"));
        assert!(tv_only_app("включи free tv"));
        assert!(tv_only_app("включи лмд"));
        assert!(!tv_only_app("включи bonobo"));
    }

    #[test]
    fn launch_script_is_valid_shell() {
        let script = launch_script("com.google.android.youtube.tv");
        assert!(script.contains("LEANBACK_LAUNCHER com.google.android.youtube.tv"));
        let ok = std::process::Command::new("sh").arg("-n").arg("-c").arg(&script).status().unwrap().success();
        assert!(ok, "{script}");
    }

    #[test]
    fn latin_names_said_in_russian() {
        let pk = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let installed = pk(&["ru.zona.tv", "com.mxtech.videoplayer.ad", "tv.twitch.android.app", "com.amazon.avod"]);
        assert_eq!(find_package("зону", &installed).as_deref(), Some("ru.zona.tv"));
        assert_eq!(find_package("zona", &installed).as_deref(), Some("ru.zona.tv"));
        assert_eq!(find_package("зона", &installed).as_deref(), Some("ru.zona.tv"));
        assert_eq!(find_package("зону", &pk(&["com.amazon.avod"])), None, "«zon» must not match amazon");
    }

    #[test]
    fn reads_launcher_apps_safely() {
        let out = "priority=0 preferredOrder=0\ncom.google.android.youtube.tv/com.google.android.apps.youtube.tv.activity.ShellActivity\nru.kinopoisk.tv/.MainActivity\nbad;rm -rf/x\n";
        assert_eq!(packages_of(out), vec!["com.google.android.youtube.tv", "ru.kinopoisk.tv"]);
        assert!(!valid_package("a;b") && !valid_package("") && valid_package("tv.okko.androidtv"));
    }

    #[test]
    fn transliterates_and_encodes() {
        assert_eq!(translit("лампа"), "lampa");
        assert_eq!(percent_encode("котики 2"), "%D0%BA%D0%BE%D1%82%D0%B8%D0%BA%D0%B8+2");
    }
}
