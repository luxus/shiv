//! CompLock's engine settings, from [deadlockconfig.com](https://deadlockconfig.com/):
//! the ConVars of their published `gameinfo.gi`, staged into the Performance
//! screen and written through [`crate::perf::apply`], and their pack options
//! — the paks and per-option convar deltas the site's Pack Builder assembles
//! into one zip. Both assets are their data verbatim, kept at the site's
//! current build by the `complock-watch` workflow; the meta beside them names
//! the build and the fetch date. Only the ConVars are taken from the
//! gameinfo — their search paths match what Shiv's own fix-mount writes, and
//! their mod paks are ordinary mods Shiv manages like any other, downloaded
//! through [`part_url`] and installed through [`crate::install::install_files`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use kv_parser::{ParseOptions, SourceDocument, SourceKind};
use serde::Deserialize;

use crate::perf::ConfigEntry;

const GAMEINFO: &str = include_str!("../assets/complock-gameinfo.gi");
const META: &str = include_str!("../assets/complock.json");
const OPTIONS: &str = include_str!("../assets/complock-options.json");

/// Where the pack data's part urls download from: they are site-relative.
pub const SITE: &str = "https://deadlockconfig.com";

/// Shown when the pack's `app_min` is newer than [`PACK_APP_VERSION`].
pub const UPDATE_NEEDED: &str = "shiv update needed";

/// The pack-data app version CompLock compares `app_min` with (`Web.AppVersion`).
pub const PACK_APP_VERSION: u64 = 1;

/// How long a successful look at the site stays current while the screen is open.
pub const PACK_REFRESH: Duration = Duration::from_secs(5 * 60);

/// The cached pack-data file inside the shiv cache directory.
pub const PACK_CACHE_FILE: &str = "pack-data.json";

/// The game build the settings were crafted for, from the site's meta.
#[must_use]
pub fn game() -> &'static str {
    build_meta().map_or("", |(game, _)| game)
}

/// When the embedded files were fetched, as `YYYY-MM-DD`.
#[must_use]
pub fn fetched() -> &'static str {
    build_meta().map_or("", |(_, fetched)| fetched)
}

/// CompLock's ConVars as staged entries: every scalar pair in their
/// `gameinfo.gi`'s `ConVars` block, in file order. The file is the only
/// source of truth; a malformed asset yields no entries, which the core
/// test on the asset catches before it reaches a build.
#[must_use]
pub fn entries() -> Vec<ConfigEntry> {
    entries_of(GAMEINFO)
}

/// The ConVars of a gameinfo text as staged entries, as [`entries`] reads
/// the embedded one.
fn entries_of(gameinfo: &str) -> Vec<ConfigEntry> {
    let Ok(doc) = SourceDocument::parse(gameinfo, parse_options()) else {
        return Vec::new();
    };
    doc.sites
        .iter()
        .filter_map(|site| {
            let SourceKind::Scalar { value, .. } = &site.kind else {
                return None;
            };
            let path: &[String] = site.path.split_first().map_or(&site.path, |(_, rest)| rest);
            (path.len() == 2 && path[0].eq_ignore_ascii_case("ConVars")).then(|| ConfigEntry {
                path: path.to_vec(),
                value: Some(value.clone()),
            })
        })
        .collect()
}

/// The parsed meta, or `None` when the workflow wrote something malformed —
/// which the core test on the asset catches before it reaches a build.
fn build_meta() -> Option<(&'static str, &'static str)> {
    static META_ONCE: std::sync::OnceLock<Option<(&'static str, &'static str)>> =
        std::sync::OnceLock::new();
    *META_ONCE.get_or_init(parse_meta)
}

fn parse_meta() -> Option<(&'static str, &'static str)> {
    let parsed: serde_json::Value = serde_json::from_str(META).ok()?;
    let game: &'static str = Box::leak(parsed.get("game")?.as_str()?.to_owned().into_boxed_str());
    let fetched: &'static str =
        Box::leak(parsed.get("fetched")?.as_str()?.to_owned().into_boxed_str());
    Some((game, fetched))
}

/// Source 2 reads backslashes literally in gameinfo.gi (as in perf).
fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_escape_sequences: false,
        ..ParseOptions::default()
    }
}

// ---------------------------------------------------------------------------
// The pack options

/// The site's `gameinfo` maps (`{"cvar": "value"}`) as staged entries, in
/// the order the JSON lists them.
fn convar_map<'de, D>(d: D) -> Result<Vec<ConfigEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Convars;
    impl<'de> serde::de::Visitor<'de> for Convars {
        type Value = Vec<ConfigEntry>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map of convar names to values")
        }
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out = Vec::new();
            while let Some((name, value)) = map.next_entry::<String, String>()? {
                out.push(ConfigEntry {
                    path: vec!["ConVars".into(), name],
                    value: Some(value),
                });
            }
            Ok(out)
        }
    }
    d.deserialize_map(Convars)
}

/// The site writes a part's `url` as one string or as the list of a big
/// pak's numbered pieces; both mean the same list.
fn url_list<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Urls {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Urls::deserialize(d)? {
        Urls::One(url) => vec![url],
        Urls::Many(urls) => urls,
    })
}

/// A date the site stamps on a card, as `YYYY-MM-DD`.
fn updated_date<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct RawUpdated {
        #[serde(default)]
        date: Option<String>,
    }
    Ok(RawUpdated::deserialize(d)?.date)
}

#[derive(Deserialize)]
struct RawDoc {
    #[serde(default)]
    base: Vec<RawBase>,
    #[serde(default)]
    group: Vec<RawGroup>,
    #[serde(default)]
    option: Vec<RawOption>,
    #[serde(default)]
    files: BTreeMap<String, RawPak>,
}

#[derive(Deserialize)]
struct RawGroup {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct RawOption {
    id: String,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    how: Option<String>,
    #[serde(default)]
    warn: Option<String>,
    #[serde(default)]
    avg: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default, deserialize_with = "updated_date")]
    updated: Option<String>,
    #[serde(default)]
    pak: Option<String>,
    #[serde(default, deserialize_with = "convar_map")]
    gameinfo: Vec<ConfigEntry>,
    #[serde(default, deserialize_with = "convar_map")]
    gameinfo_else: Vec<ConfigEntry>,
    #[serde(default)]
    choice: Vec<RawChoice>,
}

#[derive(Deserialize)]
struct RawChoice {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    warn: Option<String>,
    #[serde(default)]
    avg: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default)]
    flag: Option<String>,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    pak: Option<String>,
    #[serde(default, deserialize_with = "convar_map")]
    gameinfo: Vec<ConfigEntry>,
    #[serde(default, deserialize_with = "convar_map")]
    gameinfo_off: Vec<ConfigEntry>,
}

#[derive(Deserialize)]
struct RawBase {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    avg: Option<String>,
    #[serde(default)]
    low: Option<String>,
    #[serde(default, deserialize_with = "updated_date")]
    updated: Option<String>,
    #[serde(default)]
    pak: Option<String>,
    #[serde(default)]
    variants: BTreeMap<String, String>,
    #[serde(default)]
    choice_label: Option<String>,
    #[serde(default)]
    choice: Vec<RawChoice>,
    #[serde(default, deserialize_with = "convar_map")]
    gameinfo: Vec<ConfigEntry>,
    #[serde(default)]
    fov: Option<RawFov>,
}

#[derive(Deserialize)]
struct RawFov {
    cvar: String,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    from: f64,
    #[serde(default)]
    to: f64,
    #[serde(default)]
    by: f64,
}

#[derive(Deserialize)]
struct RawPak {
    #[serde(default)]
    title: String,
    #[serde(default)]
    parts: Vec<RawPart>,
}

#[derive(Deserialize)]
struct RawPart {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    crc: u32,
    #[serde(default)]
    csize: u64,
    #[serde(default, deserialize_with = "url_list")]
    url: Vec<String>,
}

/// One VPK file of a pak: the raw-DEFLATE stream its `urls` download — a
/// big pak arrives in numbered pieces that concatenate, in order, into the
/// one stream — and the size and CRC the decompressed VPK must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub size: u64,
    pub crc: u32,
    pub csize: u64,
    pub urls: Vec<String>,
}

/// One downloadable pak of the site's pack, as the files list describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pak {
    pub title: String,
    pub parts: Vec<Part>,
}

impl Pak {
    /// The pak's total download size: every part's compressed size added.
    #[must_use]
    pub fn download_size(&self) -> u64 {
        self.parts.iter().map(|p| p.csize).sum()
    }
}

/// A version of an option or the HUD base: its own pak and convar lines,
/// and the lines it comments out (`off_convars`) when it is the one picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub title: String,
    pub text: Option<String>,
    pub warn: Option<String>,
    pub avg: Option<String>,
    pub low: Option<String>,
    /// The HUD variant this choice swaps the base pak for.
    pub flag: Option<String>,
    pub default: bool,
    pub pak: Option<Pak>,
    pub convars: Vec<ConfigEntry>,
    pub off_convars: Vec<ConfigEntry>,
}

/// One of the site's options: a pak to install, convars to stage, or both.
/// `convars` are the lines an on option writes; `off_convars` the lines an
/// option with an explicit off state writes instead (the site's
/// `gameinfo_else`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackOption {
    pub id: String,
    pub group: Option<String>,
    pub title: String,
    pub avg: Option<String>,
    pub low: Option<String>,
    pub text: Option<String>,
    pub how: Option<String>,
    pub warn: Option<String>,
    pub updated: Option<String>,
    pub pak: Option<Pak>,
    pub convars: Vec<ConfigEntry>,
    pub off_convars: Vec<ConfigEntry>,
    pub choices: Vec<Choice>,
}

/// The HUD++ base of the pack: the pak every pack starts from, with the
/// damage-number choices that swap it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hud {
    pub id: String,
    pub title: String,
    pub text: Option<String>,
    pub avg: Option<String>,
    pub low: Option<String>,
    pub updated: Option<String>,
    pub choice_label: Option<String>,
    pub default_pak: Option<Pak>,
    pub variants: BTreeMap<String, Pak>,
    pub choices: Vec<Choice>,
}

impl Hud {
    /// The pak the choice at `index` installs: its flag's variant when one
    /// exists, else the base pak.
    #[must_use]
    pub fn pak_of_choice(&self, index: usize) -> Option<&Pak> {
        self.choices
            .get(index)
            .and_then(|c| c.flag.as_deref())
            .and_then(|flag| self.variants.get(flag))
            .or(self.default_pak.as_ref())
    }
}

/// The field of view the site's gameinfo card carries: the convar its
/// slider writes, with the range and step the site states.
#[derive(Debug, Clone, PartialEq)]
pub struct Fov {
    pub cvar: String,
    pub default: String,
    pub from: f64,
    pub to: f64,
    pub by: f64,
}

/// The gameinfo card of the pack: the convar lines every pack the site
/// builds adds on top of their published gameinfo.gi, and the FOV slider.
#[derive(Debug, Clone, PartialEq)]
pub struct GameinfoCard {
    pub id: String,
    pub title: String,
    pub text: Option<String>,
    pub updated: Option<String>,
    pub convars: Vec<ConfigEntry>,
    pub fov: Option<Fov>,
}

/// A heading the site groups its options under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub title: String,
    pub text: String,
}

/// The pack data resolved: the paks an option, choice or base names are
/// looked up in the site's files list.
#[derive(Debug, Clone, Default, PartialEq)]
struct PackData {
    hud: Option<Hud>,
    gameinfo: Option<GameinfoCard>,
    groups: Vec<Group>,
    options: Vec<PackOption>,
}

static PACK: OnceLock<PackData> = OnceLock::new();

fn pack_data() -> &'static PackData {
    PACK.get_or_init(|| parse_pack(OPTIONS).unwrap_or_default())
}

fn parse_pack(text: &str) -> Option<PackData> {
    let raw: RawDoc = serde_json::from_str(text).ok()?;
    let pak = |name: &Option<String>| {
        name.as_deref().and_then(|n| raw.files.get(n)).map(|p| Pak {
            title: p.title.clone(),
            parts: p
                .parts
                .iter()
                .map(|part| Part {
                    name: part.name.clone(),
                    size: part.size,
                    crc: part.crc,
                    csize: part.csize,
                    urls: part.url.clone(),
                })
                .collect(),
        })
    };
    let choice = |c: &RawChoice| Choice {
        id: c.id.clone(),
        title: c.title.clone(),
        text: c.text.clone(),
        warn: c.warn.clone(),
        avg: c.avg.clone(),
        low: c.low.clone(),
        flag: c.flag.clone(),
        default: c.default,
        pak: pak(&c.pak),
        convars: c.gameinfo.clone(),
        off_convars: c.gameinfo_off.clone(),
    };
    let hud = raw.base.iter().find(|b| !b.choice.is_empty());
    let gameinfo = raw.base.iter().find(|b| b.fov.is_some());
    Some(PackData {
        hud: hud.map(|hud| Hud {
            id: hud.id.clone(),
            title: hud.title.clone(),
            text: hud.text.clone(),
            avg: hud.avg.clone(),
            low: hud.low.clone(),
            updated: hud.updated.clone(),
            choice_label: hud.choice_label.clone(),
            default_pak: pak(&hud.pak),
            variants: hud
                .variants
                .iter()
                .filter_map(|(flag, name)| pak(&Some(name.clone())).map(|p| (flag.clone(), p)))
                .collect(),
            choices: hud.choice.iter().map(&choice).collect(),
        }),
        gameinfo: gameinfo.map(|b| GameinfoCard {
            id: b.id.clone(),
            title: b.title.clone(),
            text: b.text.clone(),
            updated: b.updated.clone(),
            convars: b.gameinfo.clone(),
            fov: b.fov.as_ref().map(|f| Fov {
                cvar: f.cvar.clone(),
                default: f.default.clone().unwrap_or_default(),
                from: f.from,
                to: f.to,
                by: f.by,
            }),
        }),
        groups: raw
            .group
            .iter()
            .map(|g| Group {
                id: g.id.clone(),
                title: g.title.clone(),
                text: g.text.clone(),
            })
            .collect(),
        options: raw
            .option
            .iter()
            .map(|o| PackOption {
                id: o.id.clone(),
                group: o.group.clone(),
                title: o.title.clone(),
                avg: o.avg.clone(),
                low: o.low.clone(),
                text: o.text.clone(),
                how: o.how.clone(),
                warn: o.warn.clone(),
                updated: o.updated.clone(),
                pak: pak(&o.pak),
                convars: o.gameinfo.clone(),
                off_convars: o.gameinfo_else.clone(),
                choices: o.choice.iter().map(&choice).collect(),
            })
            .collect(),
    })
}

/// Where a [`PackLoad`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackOrigin {
    /// The site's page, just now.
    Live,
    /// The last page that parsed, kept in the shiv cache.
    Cache,
    /// The `include_str!` copies shipped with this build.
    Baked,
}

/// One resolved pack: the options, the gameinfo text, and whether this Shiv
/// is older than the pack's `app_min` ([`PACK_APP_VERSION`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PackLoad {
    origin: PackOrigin,
    too_old: bool,
    game: String,
    fetched: String,
    gameinfo_text: String,
    data: PackData,
}

impl PackLoad {
    /// Where this pack was read from.
    #[must_use]
    pub fn origin(&self) -> PackOrigin {
        self.origin
    }

    /// The pack's `app_min` is a number greater than [`PACK_APP_VERSION`].
    #[must_use]
    pub fn too_old(&self) -> bool {
        self.too_old
    }

    /// The game build the settings were crafted for.
    #[must_use]
    pub fn game(&self) -> &str {
        &self.game
    }

    /// When this copy was fetched, or `live` / `cached` for a page that
    /// carries no fetch date.
    #[must_use]
    pub fn fetched(&self) -> &str {
        &self.fetched
    }

    /// The site's options, in the order the pack data lists them.
    #[must_use]
    pub fn options(&self) -> &[PackOption] {
        &self.data.options
    }

    /// The pack's group headings, in the site's order.
    #[must_use]
    pub fn groups(&self) -> &[Group] {
        &self.data.groups
    }

    /// The HUD++ base, when the pack data has one.
    #[must_use]
    pub fn hud(&self) -> Option<&Hud> {
        self.data.hud.as_ref()
    }

    /// The gameinfo card, when the pack data has one.
    #[must_use]
    pub fn gameinfo_card(&self) -> Option<&GameinfoCard> {
        self.data.gameinfo.as_ref()
    }

    /// The field of view the gameinfo card's slider writes.
    #[must_use]
    pub fn fov(&self) -> Option<&Fov> {
        self.data.gameinfo.as_ref().and_then(|c| c.fov.as_ref())
    }

    /// CompLock's ConVars as staged entries, from this pack's gameinfo text.
    #[must_use]
    pub fn entries(&self) -> Vec<ConfigEntry> {
        entries_of(&self.gameinfo_text)
    }
}

/// The pack shipped in this build. The screen shows it until a page or a
/// cache loads, and again when both of those fail.
#[must_use]
pub fn baked_pack() -> PackLoad {
    PackLoad {
        origin: PackOrigin::Baked,
        too_old: false,
        game: game().to_owned(),
        fetched: fetched().to_owned(),
        gameinfo_text: GAMEINFO.to_owned(),
        data: parse_pack(OPTIONS).unwrap_or_default(),
    }
}

/// `<user cache>/shiv`, the directory [`load_pack`] writes `pack-data.json` into.
#[must_use]
pub fn pack_cache_dir() -> Option<PathBuf> {
    crate::platform::cache_dir().map(|dir| dir.join("shiv"))
}

/// Whether another look at the site is due. `None` has never looked.
/// A look younger than [`PACK_REFRESH`] is not due.
#[must_use]
pub fn pack_refresh_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|then| now.saturating_duration_since(then) >= PACK_REFRESH)
}

/// The page at `{SITE}/`, then the cache in `cache_dir`, then [`baked_pack`].
///
/// `fetch` receives the page url and returns the HTML. A body whose
/// `<script id="pack-data">` is not an object with `files`, arrays `base`
/// and `option`, and a string `gameinfo` is the same as a failed fetch:
/// nothing is cached. A cache write that fails still returns the live pack.
/// A numeric `app_min` greater than [`PACK_APP_VERSION`] sets
/// [`PackLoad::too_old`]. A missing `app_min`, or one that is not a number,
/// does not.
#[must_use]
pub fn load_pack(
    fetch: impl FnOnce(&str) -> Result<String, String>,
    cache_dir: Option<&Path>,
) -> PackLoad {
    let url = format!("{SITE}/");
    if let Ok(html) = fetch(&url)
        && let Some(json) = script_pack_json(&html).filter(|json| pack_shape(json))
        && let Some(load) = pack_from_json(json, PackOrigin::Live)
    {
        if let Some(dir) = cache_dir {
            let _ = write_pack_cache(dir, json);
        }
        return load;
    }
    if let Some(dir) = cache_dir
        && let Ok(cached) = std::fs::read_to_string(dir.join(PACK_CACHE_FILE))
        && pack_shape(&cached)
        && let Some(load) = pack_from_json(&cached, PackOrigin::Cache)
    {
        return load;
    }
    baked_pack()
}

/// `app_min` in `pack_json` is a number greater than [`PACK_APP_VERSION`].
/// Missing, zero, a non-number, and a pack that is not the site's shape are not.
#[must_use]
pub fn app_requires_newer_shiv(pack_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(pack_json) else {
        return false;
    };
    let Some(min) = value.get("app_min") else {
        return false;
    };
    app_min_is_newer(min)
}

fn pack_from_json(json: &str, origin: PackOrigin) -> Option<PackLoad> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let data = parse_pack(json)?;
    let gameinfo_text = value.get("gameinfo")?.as_str()?.to_owned();
    let game = value
        .get("game")
        .and_then(|g| g.as_str())
        .unwrap_or("")
        .to_owned();
    let fetched = match origin {
        PackOrigin::Live => "live".to_owned(),
        PackOrigin::Cache => "cached".to_owned(),
        PackOrigin::Baked => fetched().to_owned(),
    };
    let too_old = value.get("app_min").is_some_and(app_min_is_newer);
    Some(PackLoad {
        origin,
        too_old,
        game,
        fetched,
        gameinfo_text,
        data,
    })
}

/// The reference `Checked`: `files` object, `base` and `option` arrays, `gameinfo` string.
fn pack_shape(json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    value.get("files").is_some_and(serde_json::Value::is_object)
        && value.get("base").is_some_and(serde_json::Value::is_array)
        && value.get("option").is_some_and(serde_json::Value::is_array)
        && value
            .get("gameinfo")
            .is_some_and(serde_json::Value::is_string)
}

fn script_pack_json(html: &str) -> Option<&str> {
    let start = html.find("<script id=\"pack-data\"")?;
    let rest = &html[start..];
    let open = rest.find('>')?;
    let tag = &rest[..=open];
    if !tag.contains("type=\"application/json\"") {
        return None;
    }
    let body = &rest[open + 1..];
    let end = body.find("</script>")?;
    let json = body[..end].trim();
    (!json.is_empty()).then_some(json)
}

fn app_min_is_newer(min: &serde_json::Value) -> bool {
    min.as_f64()
        .is_some_and(|version| version > PACK_APP_VERSION as f64)
}

fn write_pack_cache(dir: &Path, json: &str) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(PACK_CACHE_FILE);
    let tmp = dir.join(format!(".{PACK_CACHE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json)?;
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// The site's options, in the order the pack data lists them.
#[must_use]
pub fn options() -> &'static [PackOption] {
    &pack_data().options
}

/// The pack's group headings, in the site's order.
#[must_use]
pub fn groups() -> &'static [Group] {
    &pack_data().groups
}

/// The HUD++ base with its damage-number choices, when the pack data has one.
#[must_use]
pub fn hud() -> Option<&'static Hud> {
    pack_data().hud.as_ref()
}

/// The gameinfo card: the lines every pack adds, and the FOV slider's range.
#[must_use]
pub fn gameinfo_card() -> Option<&'static GameinfoCard> {
    pack_data().gameinfo.as_ref()
}

/// The field of view the gameinfo card's slider writes.
#[must_use]
pub fn fov() -> Option<&'static Fov> {
    pack_data().gameinfo.as_ref().and_then(|c| c.fov.as_ref())
}

/// A part's url as a full download url. The pack data carries site-relative
/// paths; an absolute url passes through, and anything else is refused.
#[must_use]
pub fn part_url(url: &str) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(url.to_owned());
    }
    url.strip_prefix('/').map(|path| format!("{SITE}/{path}"))
}

/// The raw-DEFLATE stream of a pak split into numbered pieces: the pieces
/// concatenate, in order, into the one stream — the site splits mid-stream,
/// so a piece's back-references reach into the pieces before it.
#[must_use]
pub fn assemble(pieces: &[&[u8]]) -> Vec<u8> {
    let total: usize = pieces.iter().map(|p| p.len()).sum();
    let mut stream = Vec::with_capacity(total);
    for piece in pieces {
        stream.extend_from_slice(piece);
    }
    stream
}

/// The bytes a raw-DEFLATE stream (zlib `wbits=-15`, the site's part
/// format) holds. The parts concatenate into one stream first; the result
/// is the decompressed VPK, gated separately by
/// [`verify_decompressed`].
///
/// # Errors
///
/// The stream is not valid DEFLATE, or it ends before its last block.
pub fn inflate(stream: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(stream)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The VPK a pak's downloaded piece files hold, written to `dest`: the
/// pieces read as one stream (see [`assemble`]), inflated as it copies, and
/// gated on the part's declared size, compressed size, CRC, the SHA-256
/// prefix in `url`, and the VPK signature. A mismatch removes the partial
/// `dest`. Output past `size` stops before that chunk is written. Memory
/// stays at the copy buffer: the pieces are never held whole.
///
/// # Errors
///
/// A piece file cannot be read; the concatenated stream is not valid
/// DEFLATE ([`inflate`]'s refusal); the bytes are not the declared part
/// (too large, wrong size, compressed size, CRC, SHA-256 prefix, or
/// signature); `dest` cannot be written.
pub fn unpack_pak(
    pieces: &[std::path::PathBuf],
    dest: &std::path::Path,
    crc_wanted: u32,
    size: u64,
    csize: u64,
    url: &str,
) -> Result<(), String> {
    const COPY: usize = 256 * 1024;
    const VPK_SIGNATURE: [u8; 4] = [0x34, 0x12, 0xaa, 0x55];
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let unpacked = (|| -> Result<(), String> {
        let compressed = compressed_len(pieces)?;
        let mut chain = Pieces::new(pieces)?;
        let mut decoder = flate2::read::DeflateDecoder::new(&mut chain);
        let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; COPY];
        let mut written = 0u64;
        let mut state = 0xFFFF_FFFF;
        let mut sha = Sha256::new();
        let mut head = [0u8; 4];
        loop {
            let n = decoder.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            let next = written.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
            if next > size {
                return Err("the part is larger than the site says; nothing replaced".into());
            }
            if written == 0 {
                let take = head.len().min(chunk.len());
                head[..take].copy_from_slice(&chunk[..take]);
                if head != VPK_SIGNATURE {
                    return Err("the decompressed part is not a VPK".into());
                }
            }
            std::io::Write::write_all(&mut file, chunk).map_err(|e| e.to_string())?;
            state = crc32_feed(state, chunk);
            sha.update(chunk);
            written = next;
        }
        drop(file);
        let crc = state ^ 0xFFFF_FFFF;
        if written != size {
            return Err(format!(
                "the part decompressed to {written} bytes, the pack data declares {size}"
            ));
        }
        if crc != crc_wanted {
            return Err(format!(
                "the part's CRC is {crc:#x}, the pack data declares {crc_wanted:#x}"
            ));
        }
        if compressed != csize {
            return Err(format!(
                "the compressed part is {compressed} bytes, the pack data declares {csize}"
            ));
        }
        if let Some(prefix) = url_sha_prefix(url) {
            let hex = sha_hex(&sha.finalize());
            if !hex.starts_with(prefix) {
                return Err(format!(
                    "the part's SHA-256 does not start with {prefix}; nothing replaced"
                ));
            }
        }
        Ok(())
    })();
    if unpacked.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    unpacked
}

fn compressed_len(pieces: &[std::path::PathBuf]) -> Result<u64, String> {
    let mut total = 0u64;
    for piece in pieces {
        let len = std::fs::metadata(piece).map_err(|e| e.to_string())?.len();
        total = total
            .checked_add(len)
            .ok_or_else(|| "the compressed part is larger than the counter holds".to_owned())?;
    }
    Ok(total)
}

/// The 10 hex digits CompLock reads from the first piece url
/// (`name.<10 hex>[.<digits>].z`). Absent when the url has no such suffix.
fn url_sha_prefix(url: &str) -> Option<&str> {
    let stem = url.strip_suffix(".z")?;
    let (body, tail) = stem.rsplit_once('.')?;
    if is_sha_prefix(tail) {
        return Some(tail);
    }
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hex = body.rsplit_once('.').map(|(_, hex)| hex)?;
    is_sha_prefix(hex).then_some(hex)
}

fn is_sha_prefix(hex: &str) -> bool {
    hex.len() == 10 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn sha_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    out
}

/// Several piece files read as one stream, in order.
struct Pieces {
    files: Vec<std::fs::File>,
    at: usize,
}

impl Pieces {
    fn new(paths: &[std::path::PathBuf]) -> Result<Self, String> {
        Ok(Pieces {
            files: paths
                .iter()
                .map(|p| std::fs::File::open(p).map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?,
            at: 0,
        })
    }
}

impl std::io::Read for Pieces {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at < self.files.len() {
            let n = std::io::Read::read(&mut self.files[self.at], buf)?;
            if n > 0 {
                return Ok(n);
            }
            self.at += 1;
        }
        Ok(0)
    }
}

/// CRC-32 (the IEEE polynomial the site's own pack builder uses) of
/// `bytes`, as the pack data states it for each decompressed VPK.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_feed(0xFFFF_FFFF, bytes) ^ 0xFFFF_FFFF
}

/// [`crc32`] of the file at `path`.
///
/// # Errors
///
/// The error from opening or reading the file.
pub fn crc32_file(path: &Path) -> std::io::Result<u32> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut crc = 0xFFFF_FFFF;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        crc = crc32_feed(crc, &buf[..n]);
    }
    Ok(crc ^ 0xFFFF_FFFF)
}

/// A pak already on disk whose bytes are a pack [`Part`] (length, then CRC-32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReusedPart {
    pub path: PathBuf,
    /// Set when this call renamed the file onto the part's name.
    pub moved_from: Option<String>,
    /// Chunk files that still sit beside [`Self::path`] because the part's
    /// name was already a different file. [`ReusedChunk::name`] is the name
    /// each one takes beside the part. Empty after a rename: those chunks
    /// moved with the dir VPK.
    pub chunks: Vec<ReusedChunk>,
}

/// One `pakNN_###.vpk` that belongs with a reused dir VPK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReusedChunk {
    pub path: PathBuf,
    /// The name beside the part: `pak01_000.vpk` when the part is `pak01_dir.vpk`.
    pub name: String,
}

/// Write a sidecar for pack files already on disk (a browser or zip install,
/// or a pak [`claim_reusable`] just moved into place). The files stay where
/// they are. `stamps` stays empty: Shiv did not download these bytes.
///
/// # Errors
///
/// The sidecar id or the write, as [`crate::sidecar::write`].
pub fn adopt_reused(
    addons: &Path,
    meta: &crate::install::ModMeta,
    names: &[String],
) -> Result<(), crate::sidecar::SidecarError> {
    crate::sidecar::write(
        addons,
        &crate::sidecar::Sidecar {
            mod_id: meta.mod_id.clone(),
            name: meta.name.clone(),
            version: meta.version.clone(),
            thumbnail_url: meta.thumbnail_url.clone(),
            vpks: names.to_vec(),
            nsfw: Some(meta.nsfw),
            hero: meta.hero.clone(),
            category: meta.category.clone(),
            notes: None,
            source: None,
            stamps: None,
            unverified: None,
        },
    )
}

/// Find `part`'s bytes under any `pakNN` name. Nothing is renamed.
///
/// `None` when nothing in `addons` has the part's length and CRC. When the
/// part's name is free, [`ReusedPart::moved_from`] is the name a later
/// [`claim_reusable`] would move, and [`ReusedPart::chunks`] are the chunk
/// files that would follow. When the part's name is already a different
/// file, the match stays described where it is and `moved_from` is `None`.
///
/// # Errors
///
/// Reading a candidate.
pub fn locate_reusable(addons: &Path, part: &Part) -> std::io::Result<Option<ReusedPart>> {
    let Some(found) = find_reusable(addons, part)? else {
        return Ok(None);
    };
    let dest = addons.join(&part.name);
    if found == dest {
        return Ok(Some(ReusedPart {
            path: dest,
            moved_from: None,
            chunks: Vec::new(),
        }));
    }
    let moves = chunk_moves(&found, &dest)?;
    let blocked = dest.is_file() || moves.iter().any(|(_, target)| target.is_file());
    let from = found
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned();
    Ok(Some(ReusedPart {
        path: found,
        moved_from: (!blocked).then_some(from),
        chunks: moves
            .into_iter()
            .map(|(path, target)| ReusedChunk {
                name: target
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_owned(),
                path,
            })
            .collect(),
    }))
}

/// Find `part`'s bytes under any `pakNN` name and, when the part's name is
/// free, rename them there (chunks of that number follow the dir VPK).
///
/// `None` when nothing in `addons` has the part's length and CRC. When the
/// part's name is already a different file, the match stays put: `path` is
/// that other file and `moved_from` is `None`.
///
/// # Errors
///
/// Reading a candidate or renaming the match onto a free name.
pub fn claim_reusable(addons: &Path, part: &Part) -> std::io::Result<Option<ReusedPart>> {
    let Some(found) = locate_reusable(addons, part)? else {
        return Ok(None);
    };
    let Some(from) = found.moved_from.clone() else {
        return Ok(Some(found));
    };
    let dest = addons.join(&part.name);
    let moves = found
        .chunks
        .iter()
        .map(|chunk| (chunk.path.clone(), dest.with_file_name(&chunk.name)))
        .collect::<Vec<_>>();
    rename_pak(&found.path, &dest, &moves)?;
    Ok(Some(ReusedPart {
        path: dest,
        moved_from: Some(from),
        chunks: Vec::new(),
    }))
}

fn find_reusable(addons: &Path, part: &Part) -> std::io::Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(addons) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut other = None;
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if crate::addons::parse_pak(&name).is_none() {
            continue;
        }
        let path = entry.path();
        let len = entry.metadata()?.len();
        if len != part.size {
            continue;
        }
        if crc32_file(&path)? != part.crc {
            continue;
        }
        if name.eq_ignore_ascii_case(&part.name) {
            return Ok(Some(path));
        }
        other = Some(path);
    }
    Ok(other)
}

fn rename_pak(from: &Path, dest: &Path, chunks: &[(PathBuf, PathBuf)]) -> std::io::Result<()> {
    std::fs::rename(from, dest)?;
    for (chunk, target) in chunks {
        std::fs::rename(chunk, target)?;
    }
    Ok(())
}

fn chunk_moves(from: &Path, dest: &Path) -> std::io::Result<Vec<(PathBuf, PathBuf)>> {
    let Some(from_name) = from.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let Some(dest_name) = dest.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let Some((from_n, crate::addons::PakKind::Dir)) = crate::addons::parse_pak(from_name) else {
        return Ok(Vec::new());
    };
    let Some((dest_n, _)) = crate::addons::parse_pak(dest_name) else {
        return Ok(Vec::new());
    };
    if from_n == dest_n {
        return Ok(Vec::new());
    }
    let Some(dir) = from.parent() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        let Some((n, crate::addons::PakKind::Chunk(digits))) = crate::addons::parse_pak(&name)
        else {
            continue;
        };
        if n != from_n {
            continue;
        }
        out.push((
            entry.path(),
            dir.join(format!("pak{dest_n:02}_{digits}.vpk")),
        ));
    }
    Ok(out)
}

fn crc32_feed(state: u32, bytes: &[u8]) -> u32 {
    let mut crc = state;
    for &byte in bytes {
        crc = CRC32_TABLE[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc
}

const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut crc = n as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[n] = crc;
        n += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = crc32_table();

/// Whether `bytes` are the part the pack data describes: exactly `size`
/// bytes carrying the declared `crc` of the decompressed VPK.
#[must_use]
pub fn verify_decompressed(bytes: &[u8], crc: u32, size: u64) -> bool {
    u64::try_from(bytes.len()).is_ok_and(|len| len == size) && crc32(bytes) == crc
}

// ---------------------------------------------------------------------------
// Drift: what the site publishes for an installed pak

/// Prefix of the mod ids Shiv gives the CompLock paks it installs.
pub const MOD_PREFIX: &str = "complock-";

/// The id of the HUD++ base among the CompLock mod ids (`complock-hud`).
pub const HUD_ID: &str = "hud";

/// The revision a pak's install records as its sidecar `version`: the CRCs
/// the site declares for its parts, `crc32:<hex>` joined by `+` — the site
/// changes them exactly when it changes the bytes.
#[must_use]
pub fn revision(pak: &Pak) -> String {
    pak.parts
        .iter()
        .map(|p| format!("crc32:{:08x}", p.crc))
        .collect::<Vec<_>>()
        .join("+")
}

/// What the site publishes for a Shiv-installed CompLock pak
/// (`complock-<option id>` or `complock-hud`): the revisions of every pak
/// the option or the HUD base can install (each choice is a variant), or
/// [`crate::drift::Published::Gone`] when the site no longer lists it. Any
/// other id is not CompLock's: [`crate::drift::Published::Unknown`].
#[must_use]
pub fn published(mod_id: &str) -> crate::drift::Published {
    use crate::drift::Published;
    if !mod_id.starts_with(MOD_PREFIX) {
        return Published::Unknown;
    }
    let paks = releases(mod_id);
    if paks.is_empty() {
        return Published::Gone;
    }
    Published::Current(paks.into_iter().map(revision).collect())
}

/// The pak a reset of `mod_id` reinstalls: the published pak whose
/// [`revision`] is `version` (the choice the user picked), else the default
/// (HUD++'s base pak, an option's own pak, else its first choice). `None`
/// when the pack no longer publishes the mod.
#[must_use]
pub fn release_for(mod_id: &str, version: Option<&str>) -> Option<&'static Pak> {
    let paks = releases(mod_id);
    paks.iter()
        .find(|p| version.is_some_and(|v| revision(p) == v))
        .or_else(|| paks.first())
        .copied()
}

/// The last pak slot the site's pack names: its paks carry their load
/// order in their part names (HUD++ `pak01_dir.vpk`, Particle Pruning
/// `pak02_dir.vpk`, ...), so slots `1..=last_slot()` belong to the pack and
/// Shiv installs each part on the slot it names.
#[must_use]
pub fn last_slot() -> u32 {
    let hud = hud().into_iter().flat_map(|h| {
        h.default_pak
            .iter()
            .chain(h.variants.values())
            .chain(h.choices.iter().filter_map(|c| c.pak.as_ref()))
    });
    let options = options().iter().flat_map(|o| {
        o.pak
            .iter()
            .chain(o.choices.iter().filter_map(|c| c.pak.as_ref()))
    });
    hud.chain(options)
        .flat_map(|p| &p.parts)
        .filter_map(|part| slot_of(&part.name))
        .max()
        .unwrap_or(0)
}

/// The slot a part's file name names: `pakNN_dir.vpk` → NN.
#[must_use]
pub fn slot_of(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("pak")?.strip_suffix("_dir.vpk")?;
    (digits.len() == 2).then(|| digits.parse().ok()).flatten()
}

fn releases(mod_id: &str) -> Vec<&'static Pak> {
    let Some(id) = mod_id.strip_prefix(MOD_PREFIX) else {
        return Vec::new();
    };
    if id == HUD_ID {
        hud().map_or_else(Vec::new, |h| {
            h.default_pak
                .iter()
                .chain(h.variants.values())
                .chain(h.choices.iter().filter_map(|c| c.pak.as_ref()))
                .collect()
        })
    } else {
        options()
            .iter()
            .find(|o| o.id == id)
            .map_or_else(Vec::new, |o| {
                o.pak
                    .iter()
                    .chain(o.choices.iter().filter_map(|c| c.pak.as_ref()))
                    .collect()
            })
    }
}

/// A pak the site built, recognised from the `addoninfo.txt` inside its VPK:
/// the title before the authors (`"HUD++ - HoppCX, jcStani"` → `HUD++`),
/// when the authors are CompLock's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub title: String,
    pub authors: String,
    pub build_date: Option<String>,
}

const AUTHORS: [&str; 2] = ["HoppCX", "jcStani"];

/// The CompLock identity of a dir VPK, from the `addoninfo.txt` the site's
/// pack builder writes into every pak; `None` for anything else, or a VPK
/// that cannot be read.
#[must_use]
pub fn identify(vpk: &std::path::Path) -> Option<Identity> {
    let text = addoninfo_text(vpk)?;
    let doc = SourceDocument::parse(&text, parse_options()).ok()?;
    let value = |key: &str| {
        doc.sites.iter().find_map(|site| {
            let SourceKind::Scalar { value, .. } = &site.kind else {
                return None;
            };
            site.path
                .last()
                .is_some_and(|k| k.eq_ignore_ascii_case(key))
                .then(|| value.clone())
        })
    };
    let authors = value("addonauthor")?;
    if !AUTHORS.iter().any(|a| authors.contains(a)) {
        return None;
    }
    let full = value("addontitle")?;
    let title = full
        .rsplit_once(" - ")
        .map_or(full.as_str(), |(title, _)| title)
        .trim()
        .to_owned();
    Some(Identity {
        title,
        authors,
        build_date: value("buildDate"),
    })
}

/// What the site publishes for a CompLock pak Shiv did not install, known
/// only by its title: [`crate::drift::Published::Gone`] when no pak, option
/// or base the site lists today carries that title (its parenthesised
/// variant set aside, so an older "Particle Pruning (stock tracers and
/// hits)" still matches today's), else
/// [`crate::drift::Published::Unknown`] — the site still has it, but which
/// revision is on disk cannot be told from the title.
#[must_use]
pub fn published_for_title(title: &str) -> crate::drift::Published {
    use crate::drift::Published;
    let key = |t: &str| t.split('(').next().unwrap_or(t).trim().to_ascii_lowercase();
    let wanted = key(title);
    let paks = options()
        .iter()
        .flat_map(|o| {
            std::iter::once(o.title.as_str())
                .chain(o.pak.iter().map(|p| p.title.as_str()))
                .chain(
                    o.choices
                        .iter()
                        .filter_map(|c| c.pak.as_ref())
                        .map(|p| p.title.as_str()),
                )
        })
        .chain(hud().into_iter().flat_map(|h| {
            std::iter::once(h.title.as_str())
                .chain(h.default_pak.iter().map(|p| p.title.as_str()))
                .chain(h.variants.values().map(|p| p.title.as_str()))
        }))
        .any(|t| key(t) == wanted);
    if paks {
        Published::Unknown
    } else {
        Published::Gone
    }
}

/// The text of `addoninfo.txt` in a dir VPK: its preload bytes and, when
/// the entry lives in the dir file itself (archive index 0x7fff), the
/// bytes after the tree.
fn addoninfo_text(vpk: &std::path::Path) -> Option<String> {
    use std::io::{Read as _, Seek as _};
    const IN_DIR_FILE: u16 = 0x7fff;
    const MAX_ADDONINFO: u32 = 64 * 1024;
    let header = vpk_parser::VpkParser::header_from_file(vpk).ok()?;
    let entries = vpk_parser::VpkParser::parse_directory_with_preload_from_file(vpk).ok()?;
    let (entry, preload) = entries
        .into_iter()
        .find(|(e, _)| e.filename.eq_ignore_ascii_case("addoninfo") && e.ext == "txt")?;
    let mut bytes = preload;
    if entry.entry_length > 0 {
        if entry.archive_index != IN_DIR_FILE || entry.entry_length > MAX_ADDONINFO {
            return None;
        }
        let start = vpk_parser::VpkParser::header_size(header.version)
            + u64::from(header.tree_length)
            + u64::from(entry.entry_offset);
        let mut file = std::fs::File::open(vpk).ok()?;
        file.seek(std::io::SeekFrom::Start(start)).ok()?;
        let mut rest = vec![0u8; entry.entry_length as usize];
        file.read_exact(&mut rest).ok()?;
        bytes.extend_from_slice(&rest);
    }
    String::from_utf8(bytes).ok()
}

// ---------------------------------------------------------------------------
// The watch's checks

/// What the daily watch found wrong with a fresh pack-data: each reason it
/// refuses to embed the update and opens an issue instead (#20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackProblem {
    /// The fresh pack-data does not parse into the typed model.
    Unparsed(String),
    /// The fresh gameinfo text carries no ConVars the staging can map.
    GameinfoUnmapped,
    /// A card names a pak the files list lacks.
    MissingPak { card: String, pak: String },
    /// A pak's part cannot be downloaded as declared: no url, no size.
    IncompletePart { pak: String, part: String },
    /// A key the last good data did not have, in the named section.
    NewKey { section: &'static str, key: String },
    /// A key the last good data had and the fresh one dropped.
    GoneKey { section: &'static str, key: String },
    /// An option the last good data did not have.
    NewOption(String),
    /// An option the last good data had and the site retired.
    GoneOption(String),
    /// A part that did not download, unpack or match its declared CRC/size.
    PartFailed {
        pak: String,
        part: String,
        reason: String,
    },
}

impl std::fmt::Display for PackProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackProblem::Unparsed(e) => write!(f, "pack-data does not parse: {e}"),
            PackProblem::GameinfoUnmapped => write!(f, "the gameinfo carries no ConVars to stage"),
            PackProblem::MissingPak { card, pak } => {
                write!(f, "{card} names pak `{pak}`, which files lacks")
            }
            PackProblem::IncompletePart { pak, part } => {
                write!(f, "{pak} / {part}: no url, crc or size")
            }
            PackProblem::NewKey { section, key } => write!(f, "new key `{section}.{key}`"),
            PackProblem::GoneKey { section, key } => write!(f, "key `{section}.{key}` is gone"),
            PackProblem::NewOption(id) => write!(f, "new option `{id}`"),
            PackProblem::GoneOption(id) => write!(f, "option `{id}` is gone upstream"),
            PackProblem::PartFailed { pak, part, reason } => {
                write!(f, "{pak} / {part}: {reason}")
            }
        }
    }
}

/// The fresh pack-data (without its gameinfo text) and gameinfo against the
/// last good pack-data: the typed parse Shiv itself uses, every pak a card
/// names present with downloadable parts, and the key and option sets
/// unchanged. Empty means the update is safe to embed as far as the data
/// goes; [`check_parts`] proves the bytes.
#[must_use]
pub fn check_pack(fresh: &str, gameinfo: &str, last_good: &str) -> Vec<PackProblem> {
    let mut problems = Vec::new();
    if entries_of(gameinfo).is_empty() {
        problems.push(PackProblem::GameinfoUnmapped);
    }
    let fresh_value: serde_json::Value = match serde_json::from_str(fresh) {
        Ok(v) => v,
        Err(e) => {
            problems.push(PackProblem::Unparsed(e.to_string()));
            return problems;
        }
    };
    let raw: RawDoc = match serde_json::from_value(fresh_value.clone()) {
        Ok(raw) => raw,
        Err(e) => {
            problems.push(PackProblem::Unparsed(e.to_string()));
            return problems;
        }
    };
    if raw.option.is_empty() {
        problems.push(PackProblem::Unparsed("no options".into()));
    }
    for (card, pak) in named_paks(&raw) {
        match raw.files.get(&pak) {
            None => problems.push(PackProblem::MissingPak { card, pak }),
            Some(found) => {
                for part in &found.parts {
                    if part.url.is_empty() || part.size == 0 || part.crc == 0 {
                        problems.push(PackProblem::IncompletePart {
                            pak: pak.clone(),
                            part: part.name.clone(),
                        });
                    }
                }
            }
        }
    }
    if let Ok(last_value) = serde_json::from_str::<serde_json::Value>(last_good) {
        let (mut fresh_keys, mut last_keys) = (key_sets(&fresh_value), key_sets(&last_value));
        for (section, key) in OPTIONAL_KEYS {
            for keys in [&mut fresh_keys, &mut last_keys] {
                if let Some(set) = keys.get_mut(section) {
                    set.remove(key);
                }
            }
        }
        for (section, keys) in &fresh_keys {
            let before = last_keys.get(section);
            for key in keys
                .iter()
                .filter(|k| before.is_none_or(|b| !b.contains(*k)))
            {
                problems.push(PackProblem::NewKey {
                    section,
                    key: key.clone(),
                });
            }
        }
        for (section, keys) in &last_keys {
            let now = fresh_keys.get(section);
            for key in keys.iter().filter(|k| now.is_none_or(|n| !n.contains(*k))) {
                problems.push(PackProblem::GoneKey {
                    section,
                    key: key.clone(),
                });
            }
        }
        let ids = |v: &serde_json::Value| -> std::collections::BTreeSet<String> {
            v.get("option")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| o.get("id").and_then(serde_json::Value::as_str))
                .map(str::to_owned)
                .collect()
        };
        let (fresh_ids, last_ids) = (ids(&fresh_value), ids(&last_value));
        problems.extend(
            fresh_ids
                .difference(&last_ids)
                .map(|id| PackProblem::NewOption(id.clone())),
        );
        problems.extend(
            last_ids
                .difference(&fresh_ids)
                .map(|id| PackProblem::GoneOption(id.clone())),
        );
    }
    problems
}

/// Every part the fresh pack-data declares, unpacked from the piece files
/// already downloaded into `dir` under their url's file name, against the
/// declared size and CRC ([`unpack_pak`]). A part shared by several cards is
/// checked once; a missing piece file is a failed download.
#[must_use]
pub fn check_parts(fresh: &str, dir: &std::path::Path) -> Vec<PackProblem> {
    let Ok(raw) = serde_json::from_str::<RawDoc>(fresh) else {
        return vec![PackProblem::Unparsed("pack-data does not parse".into())];
    };
    let mut problems = Vec::new();
    let mut checked = std::collections::BTreeSet::new();
    let scratch = dir.join(".unpacked");
    for (_, name) in named_paks(&raw) {
        let Some(pak) = raw.files.get(&name) else {
            continue;
        };
        for part in &pak.parts {
            if !checked.insert(part.url.clone()) {
                continue;
            }
            let pieces: Vec<std::path::PathBuf> = part
                .url
                .iter()
                .map(|url| dir.join(url.rsplit('/').next().unwrap_or(url)))
                .collect();
            let fail = |reason: String| PackProblem::PartFailed {
                pak: name.clone(),
                part: part.name.clone(),
                reason,
            };
            if let Some(missing) = pieces.iter().find(|p| !p.is_file()) {
                problems.push(fail(format!("{} did not download", missing.display())));
                continue;
            }
            if let Err(e) = std::fs::create_dir_all(&scratch) {
                problems.push(fail(e.to_string()));
                continue;
            }
            let dest = scratch.join(&part.name);
            match unpack_pak(
                &pieces,
                &dest,
                part.crc,
                part.size,
                part.csize,
                part.url.first().map(String::as_str).unwrap_or(""),
            ) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&dest);
                }
                Err(e) => problems.push(fail(e)),
            }
        }
    }
    let _ = std::fs::remove_dir(&scratch);
    problems
}

/// Every (card, pak name) pair the raw pack-data names: options, their
/// choices, the bases, their choices and the HUD variants.
fn named_paks(raw: &RawDoc) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut add = |card: &str, pak: &Option<String>| {
        if let Some(pak) = pak {
            out.push((card.to_owned(), pak.clone()));
        }
    };
    for o in &raw.option {
        add(&o.id, &o.pak);
        for c in &o.choice {
            add(&format!("{}/{}", o.id, c.id), &c.pak);
        }
    }
    for b in &raw.base {
        add(&b.id, &b.pak);
        for c in &b.choice {
            add(&format!("{}/{}", b.id, c.id), &c.pak);
        }
        for (flag, pak) in &b.variants {
            add(&format!("{}/{flag}", b.id), &Some(pak.clone()));
        }
    }
    out
}

/// Keys the typed model reads where absence is a legal state, as CompLock's
/// own client tests them with `Has`: no `gameinfo_else` is an option that
/// turns off by leaving its line out, no `gameinfo_off` a choice that
/// comments nothing out. The site adds and drops them per card (build 6860
/// dropped ragdolls' last `gameinfo_else`, #37), so their coming and going
/// is not a schema change the watch should stop on.
const OPTIONAL_KEYS: [(&str, &str); 2] = [("option", "gameinfo_else"), ("choice", "gameinfo_off")];

/// The keys each section of a pack-data carries, unioned over its entries:
/// the shape the watch compares against the last good data.
fn key_sets(doc: &serde_json::Value) -> BTreeMap<&'static str, std::collections::BTreeSet<String>> {
    use serde_json::Value;
    let mut sets: BTreeMap<&'static str, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut note = |section: &'static str, v: &Value| {
        if let Some(map) = v.as_object() {
            sets.entry(section).or_default().extend(map.keys().cloned());
        }
    };
    let list = |v: &Value, key: &str| -> Vec<Value> {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    note("top", doc);
    for (section, cards) in [("option", list(doc, "option")), ("base", list(doc, "base"))] {
        for card in &cards {
            note(section, card);
            for choice in list(card, "choice") {
                note("choice", &choice);
            }
            if let Some(fov) = card.get("fov") {
                note("fov", fov);
            }
        }
    }
    for group in list(doc, "group") {
        note("group", &group);
    }
    if let Some(files) = doc.get("files").and_then(Value::as_object) {
        for pak in files.values() {
            note("files", pak);
            for part in list(pak, "parts") {
                note("part", &part);
            }
        }
    }
    sets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meta_names_a_build_and_a_fetch_date() {
        assert!(build_meta().is_some(), "the workflow's meta parses");
        assert!(game().bytes().all(|b| b.is_ascii_digit()) && !game().is_empty());
        assert_eq!(fetched().len(), 10, "a YYYY-MM-DD date");
    }

    #[test]
    fn the_entries_are_the_convars_block_verbatim() {
        let first = entries();
        assert!(!first.is_empty(), "a CompLock gameinfo carries settings");
        for entry in &first {
            assert_eq!(entry.path.len(), 2);
            assert_eq!(entry.path[0].to_ascii_lowercase(), "convars");
            assert!(
                entry.value.as_deref().is_some_and(|v| !v.is_empty()),
                "{} carries a value",
                entry.path[1]
            );
        }
        // Re-parsing reproduces the same entries: the file is the truth.
        assert_eq!(entries(), first);
    }

    #[test]
    fn the_asset_is_a_gameinfo_with_convars_and_search_paths() {
        assert!(GAMEINFO.contains("ConVars"));
        assert!(GAMEINFO.contains("SearchPaths"));
        assert!(
            GAMEINFO.contains("citadel/addons"),
            "their mounts are intact"
        );
    }

    #[test]
    fn the_options_asset_parses_into_the_pack_data() {
        assert!(
            parse_pack(OPTIONS).is_some(),
            "the workflow's options asset parses"
        );
        assert!(
            !options().is_empty() && !groups().is_empty(),
            "the site's options and groups are there"
        );
        assert!(hud().is_some(), "the HUD++ base is there");
        assert!(gameinfo_card().is_some(), "the gameinfo card is there");
        assert!(fov().is_some(), "the gameinfo card carries the FOV field");
    }
}
