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
use std::sync::OnceLock;

use kv_parser::{ParseOptions, SourceDocument, SourceKind};
use serde::Deserialize;

use crate::perf::ConfigEntry;

const GAMEINFO: &str = include_str!("../assets/complock-gameinfo.gi");
const META: &str = include_str!("../assets/complock.json");
const OPTIONS: &str = include_str!("../assets/complock-options.json");

/// Where the pack data's part urls download from: they are site-relative.
pub const SITE: &str = "https://deadlockconfig.com";

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
    let Ok(doc) = SourceDocument::parse(GAMEINFO, parse_options()) else {
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
#[derive(Debug, Default, PartialEq)]
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
/// gated on the part's declared size, CRC and VPK signature — a mismatch
/// removes the partial `dest` and changes nothing else. Memory stays at the
/// copy buffer: the pieces are never held whole.
///
/// # Errors
///
/// A piece file cannot be read; the concatenated stream is not valid
/// DEFLATE ([`inflate`]'s refusal); the bytes are not the declared part
/// (wrong size, CRC or signature); `dest` cannot be written.
pub fn unpack_pak(
    pieces: &[std::path::PathBuf],
    dest: &std::path::Path,
    crc_wanted: u32,
    size: u64,
) -> Result<(), String> {
    const COPY: usize = 256 * 1024;
    const VPK_SIGNATURE: [u8; 4] = [0x34, 0x12, 0xaa, 0x55];
    use std::io::Read as _;
    let unpacked = (|| -> Result<u64, String> {
        let mut chain = Pieces::new(pieces)?;
        let mut decoder = flate2::read::DeflateDecoder::new(&mut chain);
        let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; COPY];
        let mut written = 0u64;
        let mut state = 0xFFFF_FFFF;
        let mut head = [0u8; 4];
        loop {
            let n = decoder.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            if written == 0 {
                let take = head.len().min(chunk.len());
                head[..take].copy_from_slice(&chunk[..take]);
                if head != VPK_SIGNATURE {
                    return Err("the decompressed part is not a VPK".into());
                }
            }
            std::io::Write::write_all(&mut file, chunk).map_err(|e| e.to_string())?;
            state = crc32_feed(state, chunk);
            written += n as u64;
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
        Ok(written)
    })();
    if unpacked.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    unpacked.map(|_| ())
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
