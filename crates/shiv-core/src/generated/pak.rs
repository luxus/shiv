//! Enough of a VPK v1 reader to pull the hitsound files back out of a pak
//! the builder wrote. The directory and the file bytes sit in the one file.

use super::templates::{self, HIT_BUILDER};

/// Volume, pitch and trimmed length read from a hitsound pak.
#[derive(Debug, Clone, PartialEq)]
pub struct HitsoundFacts {
    pub body_samples: u32,
    pub head_samples: u32,
    pub body_db: f64,
    pub head_db: f64,
    pub body_pitch: f64,
    pub head_pitch: f64,
}

/// The settings a hitsound pak still carries. The trim's start sample is
/// not in the file, only the length that was encoded.
#[must_use]
pub fn read_hitsound(bytes: &[u8]) -> Option<HitsoundFacts> {
    let files = vpk_files(bytes)?;
    let body = file(
        &files,
        "sounds/hit_indicators/damage_send_default_01.vsnd_c",
    )?;
    let head = file(&files, "sounds/hit_indicators/damage_send_crit_01.vsnd_c")?;
    let events = file(&files, "soundevents/damage.vsndevts_c")?;
    let samples = templates::offset(HIT_BUILDER, "vsnd", "samples")?;
    let (base_body, base_head, _) = templates::base_levels(HIT_BUILDER)?;
    let body_vol = templates::offset(HIT_BUILDER, "events", "body.volume")?;
    let body_pitch = templates::offset(HIT_BUILDER, "events", "body.pitch")?;
    let head_vol = templates::offset(HIT_BUILDER, "events", "head.send.volume")?;
    let head_pitch = templates::offset(HIT_BUILDER, "events", "head.send.pitch")?;
    Some(HitsoundFacts {
        body_samples: u32_at(body, samples)?,
        head_samples: u32_at(head, samples)?,
        body_db: f64_at(events, body_vol)? - base_body,
        head_db: f64_at(events, head_vol)? - base_head,
        body_pitch: f64_at(events, body_pitch)?,
        head_pitch: f64_at(events, head_pitch)?,
    })
}

fn file<'a>(files: &'a [(String, Vec<u8>)], path: &str) -> Option<&'a [u8]> {
    files
        .iter()
        .find(|(name, _)| name == path)
        .map(|(_, data)| data.as_slice())
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn f64_at(bytes: &[u8], offset: usize) -> Option<f64> {
    let end = offset.checked_add(8)?;
    Some(f64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn vpk_files(bytes: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    if u32_at(bytes, 0)? != 0x55AA_1234 {
        return None;
    }
    let version = u32_at(bytes, 4)?;
    let header: usize = if version == 1 {
        12
    } else if version == 2 {
        28
    } else {
        return None;
    };
    let tree_len = usize::try_from(u32_at(bytes, 8)?).ok()?;
    let tree_start = header;
    let data_start = tree_start.checked_add(tree_len)?;
    if data_start > bytes.len() {
        return None;
    }
    let mut p = tree_start;
    let mut out = Vec::new();
    loop {
        let ext = cstr(bytes, &mut p)?;
        if ext.is_empty() {
            break;
        }
        loop {
            let dir = cstr(bytes, &mut p)?;
            if dir.is_empty() {
                break;
            }
            loop {
                let name = cstr(bytes, &mut p)?;
                if name.is_empty() {
                    break;
                }
                let entry_end = p.checked_add(18)?;
                let entry = bytes.get(p..entry_end)?;
                let preload = usize::from(u16::from_le_bytes([entry[4], entry[5]]));
                let archive = u16::from_le_bytes([entry[6], entry[7]]);
                let off =
                    usize::try_from(u32::from_le_bytes(entry[8..12].try_into().ok()?)).ok()?;
                let len =
                    usize::try_from(u32::from_le_bytes(entry[12..16].try_into().ok()?)).ok()?;
                p = entry_end;
                if archive != 0x7FFF {
                    return None;
                }
                let pre_end = p.checked_add(preload)?;
                let pre = bytes.get(p..pre_end)?;
                p = pre_end;
                let body_start = data_start.checked_add(off)?;
                let body_end = body_start.checked_add(len)?;
                let body = bytes.get(body_start..body_end)?;
                let total = pre.len().checked_add(body.len())?;
                let mut data = Vec::with_capacity(total);
                data.extend_from_slice(pre);
                data.extend_from_slice(body);
                let path = if dir == " " {
                    format!("{name}.{ext}")
                } else {
                    format!("{dir}/{name}.{ext}")
                };
                out.push((path, data));
            }
        }
    }
    Some(out)
}

fn cstr(bytes: &[u8], at: &mut usize) -> Option<String> {
    let start = *at;
    let end = bytes[start..].iter().position(|b| *b == 0)? + start;
    let text = std::str::from_utf8(bytes.get(start..end)?).ok()?.to_owned();
    *at = end + 1;
    Some(text)
}
