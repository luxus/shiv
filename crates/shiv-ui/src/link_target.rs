//! The link classifier for the install dialog. A direct download link
//! downloads as before; a mod page from `gamebanana.com` or
//! `deadlocker.net` — or a Deadlock Mod Manager or Grimoire 1-click link —
//! names a catalog mod instead. Everything pasted is untrusted: schemes,
//! hosts, ids and slugs are validated here, before any of it resolves or
//! downloads, and every refusal carries the message the dialog shows.

/// The 1-click schemes GameBanana's buttons fire at the other managers:
/// Deadlock Mod Manager's three, and Grimoire's.
const MANAGER_SCHEMES: [&str; 4] = [
    "deadlock-mod-manager:",
    "deadlock-modmanager:",
    "dlmm:",
    "grimoire:",
];

const NOT_A_MOD_PAGE: &str =
    "That is a GameBanana page, but not a mod or sound page. Paste the mod's page link.";
const NOT_A_DEADLOCKER_MOD: &str =
    "That is a deadlocker.net page, but not a mod page. Paste the mod's link.";
const NOT_AN_INSTALL_LINK: &str =
    "That Deadlock Mod Manager or Grimoire link does not name a mod to install.";
const NOT_A_GAMEBANANA_DOWNLOAD: &str =
    "That install link names a download outside gamebanana.com, which Shiv does not install.";

/// What the install dialog does with pasted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// Download the file and install it as a local mod.
    Download { url: String },
    /// Open the catalog page of this GameBanana submission.
    GameBananaPage { audio: bool, remote_id: String },
    /// Ask deadlocker.net which GameBanana submission its page names.
    DeadlockerPage { slug: String },
    /// A DMM or Grimoire 1-click link: GameBanana's `archive,mod|sound,id`
    /// spec, the archive held to gamebanana.com as both managers check.
    ManagerInstall {
        archive_url: String,
        audio: bool,
        mod_id: Option<String>,
    },
}

/// What the pasted text names. `Ok(None)` is a local path; `Err` is a link
/// Shiv refuses, carrying the message the dialog shows.
pub(crate) fn classify(text: &str) -> Result<Option<LinkTarget>, String> {
    let t = text.trim();
    if let Some(rest) = after_scheme(t, "https://").or_else(|| after_scheme(t, "http://")) {
        return web_target(t, rest).map(Some);
    }
    for scheme in MANAGER_SCHEMES {
        if let Some(rest) = after_scheme(t, scheme) {
            return manager_target(rest).map(Some);
        }
    }
    Ok(None)
}

/// The `(is a Sound, submission id)` the gamebanana.com URL `answer` names.
/// The answer of another site is untrusted, so it is validated here before
/// anything resolves it.
pub(crate) fn gamebanana_submission(answer: &str) -> Result<(bool, String), String> {
    let t = answer.trim();
    let rest = after_scheme(t, "https://").ok_or_else(|| NOT_A_MOD_PAGE.to_owned())?;
    let (host, path) = host_and_path(rest);
    if !host_is(host, "gamebanana.com") {
        return Err(NOT_A_MOD_PAGE.into());
    }
    submission_of(path).ok_or_else(|| NOT_A_MOD_PAGE.to_owned())
}

/// A web URL: a mod page under a known host, else a direct download, as
/// any pasted link was before.
fn web_target(url: &str, rest: &str) -> Result<LinkTarget, String> {
    let (host, path) = host_and_path(rest);
    if host_is(host, "gamebanana.com") {
        return match submission_of(path) {
            Some((audio, remote_id)) => Ok(LinkTarget::GameBananaPage { audio, remote_id }),
            // A GameBanana file link (`/dl/<id>`) is a download like any other.
            None if path.starts_with("/dl/") => Ok(LinkTarget::Download {
                url: url.to_owned(),
            }),
            None => Err(NOT_A_MOD_PAGE.into()),
        };
    }
    if host_is(host, "deadlocker.net") {
        let mut parts = path.trim_start_matches('/').split('/');
        let kind = parts.next();
        let slug = parts.next().unwrap_or("");
        if kind == Some("mod") && parts.next().is_none() && valid_slug(slug) {
            return Ok(LinkTarget::DeadlockerPage {
                slug: slug.to_owned(),
            });
        }
        return Err(NOT_A_DEADLOCKER_MOD.into());
    }
    Ok(LinkTarget::Download {
        url: url.to_owned(),
    })
}

/// A DMM or Grimoire 1-click payload after the scheme. Only a GameBanana
/// archive passes, as both managers enforce on their own links.
fn manager_target(payload: &str) -> Result<LinkTarget, String> {
    let parts: Vec<&str> = payload.split(',').collect();
    if parts.len() > 3 {
        return Err(NOT_AN_INSTALL_LINK.into());
    }
    let archive = parts
        .first()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .ok_or_else(|| NOT_AN_INSTALL_LINK.to_owned())?;
    let (host, _) = host_and_path(
        after_scheme(archive, "https://").ok_or_else(|| NOT_A_GAMEBANANA_DOWNLOAD.to_owned())?,
    );
    if !host_is(host, "gamebanana.com") {
        return Err(NOT_A_GAMEBANANA_DOWNLOAD.into());
    }
    let audio = match parts.get(1).map(|p| p.trim()) {
        None | Some("") => false,
        Some(t) if t.eq_ignore_ascii_case("mod") => false,
        Some(t) if t.eq_ignore_ascii_case("sound") => true,
        Some(_) => return Err(NOT_AN_INSTALL_LINK.into()),
    };
    let mod_id = match parts.get(2).map(|p| p.trim()) {
        None | Some("") => None,
        Some(id) if valid_id(id) => Some(id.to_owned()),
        Some(_) => return Err(NOT_AN_INSTALL_LINK.into()),
    };
    Ok(LinkTarget::ManagerInstall {
        archive_url: archive.to_owned(),
        audio,
        mod_id,
    })
}

/// The text after `scheme`, when `text` starts with it. Browsers lowercase
/// schemes; pasted text may not.
fn after_scheme<'a>(text: &'a str, scheme: &str) -> Option<&'a str> {
    text.get(..scheme.len())
        .filter(|head| head.eq_ignore_ascii_case(scheme))
        .map(|_| &text[scheme.len()..])
}

/// `(host, path)` of the text after a scheme: the path stops at the first
/// `?` or `#`.
fn host_and_path(rest: &str) -> (&str, &str) {
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(cut);
    let path = match tail.find(['?', '#']) {
        Some(i) => &tail[..i],
        None => tail,
    };
    (host, path)
}

/// `host` equals `domain` or is a subdomain of it, case-insensitively; the
/// dot keeps `evilgamebanana.com` out. Byte slicing stays inside the guard.
fn host_is(host: &str, domain: &str) -> bool {
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    let h = host.as_bytes();
    let d = domain.as_bytes();
    h.len() > d.len() + 1
        && h[h.len() - d.len()..].eq_ignore_ascii_case(d)
        && h[h.len() - d.len() - 1] == b'.'
}

/// The `(is a Sound, submission id)` of a gamebanana.com page path:
/// `mods/<id>` or `sounds/<id>`, nothing deeper.
fn submission_of(path: &str) -> Option<(bool, String)> {
    let mut parts = path.trim_start_matches('/').split('/');
    let kind = parts.next()?;
    let id = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let audio = match kind {
        "mods" => false,
        "sounds" => true,
        _ => return None,
    };
    valid_id(id).then(|| (audio, id.to_owned()))
}

/// A GameBanana submission id: digits, at most ten.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 10 && id.bytes().all(|b| b.is_ascii_digit())
}

/// A deadlocker.net mod page slug is one URL segment: letters, digits, `_`,
/// `-` or `.`; `.` and `..` never name a page.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug != "."
        && slug != ".."
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(url: &str) -> LinkTarget {
        LinkTarget::Download {
            url: url.to_owned(),
        }
    }

    #[test]
    fn plain_paths_are_not_links() {
        for path in [
            "/home/kai/Haze Skin.vpk",
            "file:///tmp/a.vpk",
            "",
            "mod.vpk",
        ] {
            assert_eq!(classify(path), Ok(None), "{path:?}");
        }
    }

    #[test]
    fn direct_download_links_download_as_before() {
        assert_eq!(
            classify("https://host/Haze Skin.vpk"),
            Ok(Some(download("https://host/Haze Skin.vpk")))
        );
        assert_eq!(
            classify(" http://host/mod.zip "),
            Ok(Some(download("http://host/mod.zip")))
        );
        assert_eq!(
            classify("https://files.gamebanana.com/dl/x.zip"),
            Ok(Some(download("https://files.gamebanana.com/dl/x.zip")))
        );
        assert_eq!(
            classify("https://notgamebanana.com/mods/1"),
            Ok(Some(download("https://notgamebanana.com/mods/1")))
        );
    }

    #[test]
    fn gamebanana_pages_carry_their_submission() {
        assert_eq!(
            classify("https://gamebanana.com/mods/723598"),
            Ok(Some(LinkTarget::GameBananaPage {
                audio: false,
                remote_id: "723598".into()
            }))
        );
        assert_eq!(
            classify("https://www.gamebanana.com/sounds/95493?x=1#top"),
            Ok(Some(LinkTarget::GameBananaPage {
                audio: true,
                remote_id: "95493".into()
            }))
        );
        assert_eq!(
            classify("https://GAMEBANANA.com/mods/1"),
            Ok(Some(LinkTarget::GameBananaPage {
                audio: false,
                remote_id: "1".into()
            }))
        );
    }

    #[test]
    fn other_gamebanana_pages_refuse_instead_of_downloading() {
        for page in [
            "https://gamebanana.com/tools/22583",
            "https://gamebanana.com/mods/1/extra",
            "https://gamebanana.com/mods/1a",
            "https://gamebanana.com/mods/12345678901",
            "https://gamebanana.com/mods",
            "https://gamebanana.com/",
        ] {
            let refused = classify(page);
            assert!(
                matches!(&refused, Err(refusal) if refusal.contains("not a mod")),
                "{page} was not refused: {refused:?}"
            );
        }
    }

    #[test]
    fn a_gamebanana_file_link_still_downloads() {
        assert_eq!(
            classify("https://gamebanana.com/dl/12345"),
            Ok(Some(download("https://gamebanana.com/dl/12345")))
        );
    }

    #[test]
    fn host_lookalikes_never_pass_for_pages() {
        // A userinfo or backslash trick stays a download at worst, never a
        // trusted page.
        for lookalike in [
            "https://evil.com@gamebanana.com/mods/1",
            "https://gamebanana.com@evil.com/mods/1",
            "https://gamebanana.com\\@evil.com/mods/1",
            "https://evilgamebanana.com/mods/1",
            "https://gamebanana.com.evil.test/mods/1",
        ] {
            assert_eq!(
                classify(lookalike),
                Ok(Some(download(lookalike))),
                "{lookalike} reached the page rules"
            );
        }
    }

    #[test]
    fn deadlocker_pages_carry_their_slug() {
        assert_eq!(
            classify("https://deadlocker.net/mod/mod_1770149787580"),
            Ok(Some(LinkTarget::DeadlockerPage {
                slug: "mod_1770149787580".into()
            }))
        );
        assert_eq!(
            classify("https://www.deadlocker.net/mod/mod-1"),
            Ok(Some(LinkTarget::DeadlockerPage {
                slug: "mod-1".into()
            }))
        );
    }

    #[test]
    fn other_deadlocker_pages_refuse() {
        for page in [
            "https://deadlocker.net/character/haze",
            "https://deadlocker.net/mod/",
            "https://deadlocker.net/mod/../x",
            "https://deadlocker.net/mod/a b",
            "https://deadlocker.net/mod/..",
            "https://deadlocker.net/mod",
            "https://deadlocker.net/mod/mod_1/extra",
        ] {
            let refused = classify(page);
            assert!(
                matches!(&refused, Err(refusal) if refusal.contains("deadlocker.net")),
                "{page} was not refused: {refused:?}"
            );
        }
    }

    #[test]
    fn manager_links_parse_gamebananas_one_click_payload() {
        for scheme in MANAGER_SCHEMES {
            let link = format!("{scheme}https://gamebanana.com/dl/1234,mod,723598");
            assert_eq!(
                classify(&link),
                Ok(Some(LinkTarget::ManagerInstall {
                    archive_url: "https://gamebanana.com/dl/1234".into(),
                    audio: false,
                    mod_id: Some("723598".into())
                })),
                "{scheme}"
            );
        }
        assert_eq!(
            classify("deadlock-mod-manager:https://www.gamebanana.com/dl/9,Sound,650634"),
            Ok(Some(LinkTarget::ManagerInstall {
                archive_url: "https://www.gamebanana.com/dl/9".into(),
                audio: true,
                mod_id: Some("650634".into())
            }))
        );
        // Grimoire accepts a bare archive; the id is then unknown.
        assert_eq!(
            classify("grimoire:https://files.gamebanana.com/x.zip"),
            Ok(Some(LinkTarget::ManagerInstall {
                archive_url: "https://files.gamebanana.com/x.zip".into(),
                audio: false,
                mod_id: None
            }))
        );
        // Scheme case is the browser's, not the payload's.
        assert_eq!(
            classify("DLMM:https://gamebanana.com/dl/1,mod,2"),
            Ok(Some(LinkTarget::ManagerInstall {
                archive_url: "https://gamebanana.com/dl/1".into(),
                audio: false,
                mod_id: Some("2".into())
            }))
        );
    }

    #[test]
    fn manager_links_outside_the_install_spec_refuse() {
        for link in [
            "grimoire:forge/launch",
            "dlmm://forge/launch",
            "dlmm://auth/callback?code=x",
            "deadlock-mod-manager:registerKey,1,secret",
            "dlmm:",
            "dlmm:https://evil.com/x,mod,1",
            "dlmm:http://gamebanana.com/x,mod,1",
            "dlmm:https://gamebanana.com/x,skin,1",
            "dlmm:https://gamebanana.com/x,mod,12a7",
            "dlmm:https://gamebanana.com/x,mod,1,extra",
        ] {
            let refused = classify(link);
            assert!(refused.is_err(), "{link} was not refused: {refused:?}");
        }
    }

    #[test]
    fn submission_answers_are_validated_before_use() {
        assert_eq!(
            gamebanana_submission("https://gamebanana.com/mods/650634"),
            Ok((false, "650634".into()))
        );
        assert_eq!(
            gamebanana_submission(" https://gamebanana.com/sounds/1 "),
            Ok((true, "1".into()))
        );
        for refused in [
            "https://evil.test/mods/1",
            "http://gamebanana.com/mods/1",
            "https://gamebanana.com/tools/1",
            "not a link",
            "",
        ] {
            assert!(
                matches!(gamebanana_submission(refused), Err(refusal) if refusal.contains("not a mod")),
                "{refused:?} passed as a submission"
            );
        }
    }
}
