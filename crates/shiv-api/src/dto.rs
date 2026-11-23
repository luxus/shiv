//! Wire types for the Deadlock Mods API. Only fields the Browse/Install
//! screens need are modelled; everything non-essential is optional so an API
//! addition or a null never breaks parsing. Unknown fields are ignored.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::http::deserialize_null_default;

/// One mod from `GET /v2/mods`. `id`, `remote_id` and `name` are required: a row
/// missing any of them is invalid. A null in a non-Option field also makes the
/// row invalid (`list_mods` skips such rows). Dates are kept as the ISO-8601 strings the API
/// sends: they sort correctly as text and avoid a date dependency.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModDto {
    /// Remote (`GameBanana`) id; this is what `/v2/mods/{id}/downloads` takes.
    pub remote_id: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub remote_url: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub download_count: u64,
    #[serde(default)]
    pub downloadable: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub hero: Option<String>,
    #[serde(default)]
    pub is_audio: bool,
    #[serde(default)]
    pub is_map: bool,
    #[serde(default, rename = "isNSFW")]
    pub is_nsfw: bool,
    #[serde(default)]
    pub remote_added_at: Option<String>,
    #[serde(default)]
    pub remote_updated_at: Option<String>,
    #[serde(default)]
    pub files_updated_at: Option<String>,
    /// The author's catalog id (`/v2/mod-authors/{id}`); groups a mod's author's mods.
    #[serde(default)]
    pub mod_author_id: Option<String>,
    #[serde(default)]
    pub metadata: Option<ModMetadata>,
}

/// Extra per-mod data. Every part is optional and lenient: a bad entry here
/// must never invalidate the mod row.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModMetadata {
    #[serde(default)]
    pub donation_links: Option<Vec<DonationLink>>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct DonationLink {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
}

const DONATION_HOSTS: &[&str] = &[
    "ko-fi.com",
    "patreon.com",
    "buymeacoffee.com",
    "paypal.com",
    "paypal.me",
    "liberapay.com",
];

/// True for an `https` URL on a known donation host (a `www.` prefix is
/// fine). Ports, credentials, whitespace and control characters are refused.
#[must_use]
pub fn is_donation_url(url: &str) -> bool {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() || host.contains(['@', ':', '\\']) {
        return false;
    }
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    DONATION_HOSTS.contains(&host)
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

impl ModDto {
    /// The first donation link the author offers on a known host, as
    /// `(platform, url)`. The platform falls back to `"Support the author"`.
    #[must_use]
    pub fn donation_link(&self) -> Option<(&str, &str)> {
        let links = self.metadata.as_ref()?.donation_links.as_ref()?;
        links.iter().find_map(|l| {
            let url = l.url.as_deref()?.trim();
            is_donation_url(url).then(|| {
                let platform = l
                    .platform
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty());
                (platform.unwrap_or("Support the author"), url)
            })
        })
    }

    /// The mod's file version: the date its files changed, else its `GameBanana`
    /// update date. Shiv records this in the sidecar at install time and
    /// compares it with the catalog to spot updates. Empty strings count as missing.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        non_empty(self.files_updated_at.as_deref())
            .or_else(|| non_empty(self.remote_updated_at.as_deref()))
    }

    /// When the mod last changed: the `GameBanana` update date, else the date
    /// its files changed. Empty strings count as missing.
    #[must_use]
    pub fn last_updated(&self) -> Option<&str> {
        non_empty(self.remote_updated_at.as_deref())
            .or_else(|| non_empty(self.files_updated_at.as_deref()))
    }
}

/// One downloadable file from `GET /v2/mods/{id}/downloads`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFile {
    pub url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    /// Bytes; the API reports 0 for override downloads of unknown size.
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub md5_checksum: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DownloadsResponse {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub downloads: Vec<DownloadFile>,
}

pub(crate) struct CatalogRows(pub Vec<ModDto>);

impl<'de> Deserialize<'de> for CatalogRows {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RowsVisitor;

        impl<'de> Visitor<'de> for RowsVisitor {
            type Value = CatalogRows;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON array of catalog mods")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut mods = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(row) = seq.next_element::<StreamingRow>()? {
                    if let Some(m) = row.0 {
                        mods.push(m);
                    }
                }
                Ok(CatalogRows(mods))
            }
        }

        deserializer.deserialize_seq(RowsVisitor)
    }
}

struct StreamingRow(Option<ModDto>);

impl<'de> Deserialize<'de> for StreamingRow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StreamingRowVisitor)
    }
}

struct StreamingRowVisitor;

impl<'de> Visitor<'de> for StreamingRowVisitor {
    type Value = StreamingRow;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a catalog mod object")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(StreamingRow(None))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(StreamingRow(None))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut dto = ModDto::default();
        let mut has_id = false;
        let mut has_remote_id = false;
        let mut has_name = false;
        let mut valid = true;

        while let Some(key) = map.next_key::<FieldKey<'_>>()? {
            if !valid {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            match key.0.as_ref() {
                "id" => match map.next_value::<StrictString>()?.0 {
                    Some(v) => {
                        dto.id = v;
                        has_id = true;
                    }
                    None => valid = false,
                },
                "remoteId" => match map.next_value::<StrictString>()?.0 {
                    Some(v) => {
                        dto.remote_id = v;
                        has_remote_id = true;
                    }
                    None => valid = false,
                },
                "name" => match map.next_value::<StrictString>()?.0 {
                    Some(v) => {
                        dto.name = v;
                        has_name = true;
                    }
                    None => valid = false,
                },
                "description" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.description = v,
                    None => valid = false,
                },
                "remoteUrl" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.remote_url = v,
                    None => valid = false,
                },
                "category" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.category = v,
                    None => valid = false,
                },
                "author" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.author = v,
                    None => valid = false,
                },
                "likes" => match map.next_value::<StrictU64>()?.0 {
                    Some(v) => dto.likes = v,
                    None => valid = false,
                },
                "downloadCount" => match map.next_value::<StrictU64>()?.0 {
                    Some(v) => dto.download_count = v,
                    None => valid = false,
                },
                "downloadable" => match map.next_value::<StrictBool>()?.0 {
                    Some(v) => dto.downloadable = v,
                    None => valid = false,
                },
                "tags" => match map.next_value::<StrictStringVec>()?.0 {
                    Some(v) => dto.tags = v,
                    None => valid = false,
                },
                "images" => match map.next_value::<StrictStringVec>()?.0 {
                    Some(v) => dto.images = v,
                    None => valid = false,
                },
                "thumbnailUrl" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.thumbnail_url = v,
                    None => valid = false,
                },
                "hero" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.hero = v,
                    None => valid = false,
                },
                "isAudio" => match map.next_value::<StrictBool>()?.0 {
                    Some(v) => dto.is_audio = v,
                    None => valid = false,
                },
                "isMap" => match map.next_value::<StrictBool>()?.0 {
                    Some(v) => dto.is_map = v,
                    None => valid = false,
                },
                "isNSFW" => match map.next_value::<StrictBool>()?.0 {
                    Some(v) => dto.is_nsfw = v,
                    None => valid = false,
                },
                "remoteAddedAt" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.remote_added_at = v,
                    None => valid = false,
                },
                "remoteUpdatedAt" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.remote_updated_at = v,
                    None => valid = false,
                },
                "filesUpdatedAt" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.files_updated_at = v,
                    None => valid = false,
                },
                "modAuthorId" => match map.next_value::<OptString>()?.0 {
                    Some(v) => dto.mod_author_id = v,
                    None => valid = false,
                },
                "metadata" => match map.next_value::<OptMetadata>()?.0 {
                    Some(v) => dto.metadata = v,
                    None => valid = false,
                },
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        if valid && has_id && has_remote_id && has_name {
            Ok(StreamingRow(Some(dto)))
        } else {
            Ok(StreamingRow(None))
        }
    }
}

struct FieldKey<'de>(std::borrow::Cow<'de, str>);

impl<'de> Deserialize<'de> for FieldKey<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct KeyVisitor;
        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = FieldKey<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object key")
            }
            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
                Ok(FieldKey(std::borrow::Cow::Borrowed(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(FieldKey(std::borrow::Cow::Owned(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(FieldKey(std::borrow::Cow::Owned(v)))
            }
        }
        deserializer.deserialize_str(KeyVisitor)
    }
}

fn drain_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

fn drain_map<'de, A: MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}

struct StrictString(Option<String>);
impl<'de> Deserialize<'de> for StrictString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictString;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictString(Some(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictString(Some(v)))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(StrictString(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(StrictString(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(StrictString(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(StrictString(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictString(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(StrictString(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(StrictString(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct OptString(Option<Option<String>>);
impl<'de> Deserialize<'de> for OptString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OptString;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("optional string")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(OptString(Some(None)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(OptString(Some(Some(v.to_owned()))))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(OptString(Some(Some(v))))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OptString(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OptString(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OptString(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OptString(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(OptString(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(OptString(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct StrictU64(Option<u64>);
impl<'de> Deserialize<'de> for StrictU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictU64;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("u64")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictU64(Some(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictU64(u64::try_from(v).ok()))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(StrictU64(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(StrictU64(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(StrictU64(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictU64(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(StrictU64(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(StrictU64(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct StrictBool(Option<bool>);
impl<'de> Deserialize<'de> for StrictBool {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictBool;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bool")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictBool(Some(v)))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(StrictBool(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(StrictBool(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(StrictBool(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(StrictBool(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictBool(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(StrictBool(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(StrictBool(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct StrictStringVec(Option<Vec<String>>);
impl<'de> Deserialize<'de> for StrictStringVec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictStringVec;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("array of strings")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                let mut valid = true;
                while let Some(elem) = seq.next_element::<StrictString>()? {
                    match elem.0 {
                        Some(s) if valid => out.push(s),
                        _ => valid = false,
                    }
                }
                Ok(StrictStringVec(valid.then_some(out)))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictStringVec(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(StrictStringVec(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct OptMetadata(Option<Option<ModMetadata>>);
impl<'de> Deserialize<'de> for OptMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OptMetadata;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("optional metadata object")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(OptMetadata(Some(None)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut donation_links = None;
                let mut valid = true;
                while let Some(key) = map.next_key::<FieldKey<'_>>()? {
                    if !valid {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    }
                    if key.0.as_ref() == "donationLinks" {
                        match map.next_value::<OptDonationLinks>()?.0 {
                            Some(v) => donation_links = v,
                            None => valid = false,
                        }
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(OptMetadata(
                    valid.then_some(Some(ModMetadata { donation_links })),
                ))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OptMetadata(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OptMetadata(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OptMetadata(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OptMetadata(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(OptMetadata(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(OptMetadata(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct OptDonationLinks(Option<Option<Vec<DonationLink>>>);
impl<'de> Deserialize<'de> for OptDonationLinks {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OptDonationLinks;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("optional array of donation links")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(Some(None)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut links = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                let mut valid = true;
                while let Some(item) = seq.next_element::<StrictDonationLink>()? {
                    match item.0 {
                        Some(link) if valid => links.push(link),
                        _ => valid = false,
                    }
                }
                Ok(OptDonationLinks(valid.then_some(Some(links))))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(OptDonationLinks(None))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                drain_map(map)?;
                Ok(OptDonationLinks(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

struct StrictDonationLink(Option<DonationLink>);
impl<'de> Deserialize<'de> for StrictDonationLink {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictDonationLink;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("donation link object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut link = DonationLink::default();
                let mut valid = true;
                while let Some(key) = map.next_key::<FieldKey<'_>>()? {
                    if !valid {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    }
                    match key.0.as_ref() {
                        "url" => match map.next_value::<OptString>()?.0 {
                            Some(v) => link.url = v,
                            None => valid = false,
                        },
                        "platform" => match map.next_value::<OptString>()?.0 {
                            Some(v) => link.platform = v,
                            None => valid = false,
                        },
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(StrictDonationLink(valid.then_some(link)))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictDonationLink(None))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
                drain_seq(seq)?;
                Ok(StrictDonationLink(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn donation_urls_need_https_and_a_known_host() {
        assert!(is_donation_url("https://ko-fi.com/lilotty"));
        assert!(is_donation_url("https://www.patreon.com/x"));
        assert!(is_donation_url("https://PayPal.me/x"));
        assert!(!is_donation_url("http://ko-fi.com/x"));
        assert!(!is_donation_url("https://evil.test/ko-fi.com"));
        assert!(!is_donation_url("https://ko-fi.com@evil.test/x"));
        assert!(!is_donation_url("https://ko-fi.com.evil.test/x"));
        assert!(!is_donation_url("https://ko-fi.com:444/x"));
        assert!(!is_donation_url("https://ko-fi.com/x\nbar"));
        assert!(!is_donation_url("https://ko-fi.com/x y"));
        assert!(!is_donation_url("javascript:alert(1)"));
        assert!(!is_donation_url(""));
    }

    #[test]
    fn a_mod_offers_its_first_safe_donation_link() {
        let m: ModDto = serde_json::from_str(
            r#"{"id":"a","remoteId":"1","name":"n","modAuthorId":"auth",
            "metadata":{"donationLinks":[{"url":"https://evil.test/x","platform":"Ko-fi"},
            {"platform":"Patreon"},{"url":"https://ko-fi.com/me","platform":"Ko-fi"}]}}"#,
        )
        .unwrap();
        assert_eq!(m.donation_link(), Some(("Ko-fi", "https://ko-fi.com/me")));
        assert_eq!(m.mod_author_id.as_deref(), Some("auth"));
        let none: ModDto = serde_json::from_str(
            r#"{"id":"a","remoteId":"1","name":"n","metadata":{"donationLinks":null}}"#,
        )
        .unwrap();
        assert_eq!(none.donation_link(), None);
    }

    #[test]
    fn streaming_catalog_rows_skips_malformed_entries_without_failing() {
        let rows: CatalogRows = serde_json::from_str(
            r#"[
                {"id":"ok","remoteId":"1","name":"Good","likes":5,"unknown":{"nested":[1,2]}},
                {"id":"missing-name","remoteId":"2"},
                {"id":"null-likes","remoteId":"3","name":"Bad","likes":null},
                42,
                null,
                {"id":"ok2","remoteId":"4","name":"Good 2","tags":["a","b"]}
            ]"#,
        )
        .unwrap();
        assert_eq!(rows.0.len(), 2);
        assert_eq!(rows.0[0].id, "ok");
        assert_eq!(rows.0[1].id, "ok2");
    }
}
