#!/usr/bin/env python3
"""Decide and apply one patch release for an embedded data change.

The CompLock watch commits new pack data on main and must cut one Shiv
patch for that commit. A second run of the same pack must not bump, tag,
or dispatch again. The plan is a pure function of the facts the workflow
can read (the version in Cargo.toml, whether that version is already
tagged and released, whether this run's pack bytes differ). Applying the
plan rewrites the version the release workflow already ships: the shiv-ui
package version, its lockfile entry, and one Flatpak metainfo release.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

MARKER = "Data-Release-Version"
PACKAGE = "shiv-ui"


class ReleaseError(Exception):
    pass


def parse_version(version: str) -> str:
    parts = version.split(".")
    if len(parts) != 3 or any(not part.isdigit() for part in parts):
        raise ReleaseError(f"version is not major.minor.patch: {version}")
    canonical = ".".join(str(int(part)) for part in parts)
    if canonical != version:
        raise ReleaseError(f"version is not major.minor.patch: {version}")
    return version


def bump_patch(version: str) -> str:
    major, minor, patch = (int(part) for part in parse_version(version).split("."))
    return f"{major}.{minor}.{patch + 1}"


def bump_minor(version: str) -> str:
    major, minor, _patch = (int(part) for part in parse_version(version).split("."))
    return f"{major}.{minor + 1}.0"


def bump_major(version: str) -> str:
    major, _minor, _patch = (int(part) for part in parse_version(version).split("."))
    return f"{major + 1}.0.0"


def next_version(*, current: str, subjects: list[str]) -> str:
    """One line: `none`, or the next version for the unreleased subjects.

    A breaking line (a `!` before the colon, or a `BREAKING CHANGE:`
    line) bumps the major, any `feat` the minor, any `fix` or `perf` the
    patch. Subjects without a type before the colon (a `release 0.7.0`,
    prose) and the internal types (chore, ci, test, docs) release
    nothing. The workflow may feed body lines beside subjects; a prose
    line can only ever be misread as a feat or a breaking note, which
    over-bumps — the release PR names the version it picked, so a human
    sees it.
    """
    parse_version(current)
    breaking = False
    feat = False
    fixish = False
    for subject in subjects:
        if subject.startswith("BREAKING CHANGE"):
            breaking = True
            continue
        head, sep, _tail = subject.partition(":")
        if not sep:
            continue
        head = head.strip()
        if head.endswith("!"):
            breaking = True
            head = head[:-1]
        kind = head.split("(", 1)[0].strip()
        if kind == "feat":
            feat = True
        elif kind in ("fix", "perf"):
            fixish = True
    if breaking:
        return bump_major(current)
    if feat:
        return bump_minor(current)
    if fixish:
        return bump_patch(current)
    return "none"


def cmp_version(*, current: str, tag: str) -> str:
    """One line: `same`, `ahead`, or `behind`. `ahead` means the version
    files name a release no tag has claimed yet."""
    cur = tuple(int(part) for part in parse_version(current).split("."))
    tagged = tuple(int(part) for part in parse_version(tag.removeprefix("v")).split("."))
    if cur == tagged:
        return "same"
    return "ahead" if cur > tagged else "behind"


def marker(version: str) -> str:
    return f"{MARKER}: {parse_version(version)}"


def plan(
    *,
    assets_changed: bool,
    version: str,
    tag_exists: bool,
    release_exists: bool,
    dispatch_settled: bool,
    head_pending: bool,
    tag_is_data_release: bool,
) -> str:
    """One line: skip, commit VERSION, tag VERSION, or dispatch VERSION.

    commit means write the version files, commit, tag, and dispatch.
    tag means the commit already landed: tag it and dispatch.
    dispatch means the tag exists and the GitHub release does not.
    """
    current = parse_version(version)
    if assets_changed:
        if tag_exists or release_exists:
            return f"commit {bump_patch(current)}"
        return f"commit {current}"
    if tag_exists and release_exists:
        return "skip"
    if head_pending and not tag_exists and not release_exists:
        return f"tag {current}"
    if tag_exists and tag_is_data_release and not release_exists and not dispatch_settled:
        return f"dispatch {current}"
    return "skip"


def package_version(text: str) -> str:
    section = ""
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            continue
        if section != "[package]":
            continue
        key, sep, value = stripped.partition("=")
        if sep and key.strip() == "version":
            return _unquote(value.strip())
    raise ReleaseError("Cargo.toml has no package version")


def set_package_version(text: str, new: str) -> str:
    parse_version(new)
    lines = text.splitlines(keepends=True)
    section = ""
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            section = stripped
            continue
        if section != "[package]":
            continue
        key, sep, _value = stripped.partition("=")
        if not sep or key.strip() != "version":
            continue
        ending = "\n" if line.endswith("\n") else ""
        prefix = line[: len(line) - len(line.lstrip(" "))]
        lines[index] = f'{prefix}version = "{new}"{ending}'
        return "".join(lines)
    raise ReleaseError("Cargo.toml has no package version")


def lock_package_version(text: str, package: str) -> str:
    _name, version = _lock_package(text, package)
    return version


def set_lock_package_version(text: str, package: str, new: str) -> str:
    parse_version(new)
    lines = text.splitlines(keepends=True)
    index = 0
    while index < len(lines):
        if lines[index].strip() != "[[package]]":
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end].strip() != "[[package]]":
            end += 1
        name = None
        version_at = None
        for cursor in range(index + 1, end):
            stripped = lines[cursor].strip()
            key, sep, value = stripped.partition("=")
            if not sep:
                continue
            if key.strip() == "name" and name is None:
                name = _unquote(value.strip())
            elif key.strip() == "version" and version_at is None:
                version_at = cursor
        if name == package:
            if version_at is None:
                raise ReleaseError(f"the lockfile entry {package} has no version")
            line = lines[version_at]
            ending = "\n" if line.endswith("\n") else ""
            prefix = line[: len(line) - len(line.lstrip(" "))]
            lines[version_at] = f'{prefix}version = "{new}"{ending}'
            return "".join(lines)
        index = end
    raise ReleaseError(f"the lockfile has no package {package}")


def insert_release(text: str, version: str, date: str, summary: str) -> str:
    parse_version(version)
    if len(date) != 10 or date[4] != "-" or date[7] != "-" or not date.replace("-", "").isdigit():
        raise ReleaseError(f"date is not YYYY-MM-DD: {date}")
    start = text.find("<releases>")
    end = text.find("</releases>")
    if start < 0 or end < 0 or end < start:
        raise ReleaseError("metainfo has no releases list")
    block = text[start:end]
    if f'version="{version}"' in block:
        return text
    body = summary.strip()
    if body:
        entry = (
            f'    <release version="{version}" date="{date}">\n'
            f"      <description>\n"
            f"        <p>{_xml(body)}</p>\n"
            f"      </description>\n"
            f"    </release>\n"
        )
    else:
        entry = f'    <release version="{version}" date="{date}"/>\n'
    at = start + len("<releases>")
    if text[at : at + 1] == "\n":
        at += 1
    return text[:at] + entry + text[at:]


def apply_tree(
    cargo_text: str,
    lock_text: str,
    metainfo_text: str,
    *,
    src: str,
    dest: str,
    date: str,
    summary: str,
) -> tuple[str, str, str]:
    old = parse_version(src)
    new = parse_version(dest)
    current = package_version(cargo_text)
    locked = lock_package_version(lock_text, PACKAGE)
    if current not in (old, new) or locked not in (old, new):
        raise ReleaseError(
            f"refusing to rewrite {current} / lock {locked} toward {old} -> {new}"
        )
    cargo = cargo_text if current == new else set_package_version(cargo_text, new)
    lock = lock_text if locked == new else set_lock_package_version(lock_text, PACKAGE, new)
    meta = insert_release(metainfo_text, new, date, summary)
    return cargo, lock, meta


def _lock_package(text: str, package: str) -> tuple[str, str]:
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        if lines[index].strip() != "[[package]]":
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end].strip() != "[[package]]":
            end += 1
        name = None
        version = None
        for cursor in range(index + 1, end):
            key, sep, value = lines[cursor].strip().partition("=")
            if not sep:
                continue
            if key.strip() == "name" and name is None:
                name = _unquote(value.strip())
            elif key.strip() == "version" and version is None:
                version = _unquote(value.strip())
        if name == package:
            if version is None:
                raise ReleaseError(f"the lockfile entry {package} has no version")
            return name, version
        index = end
    raise ReleaseError(f"the lockfile has no package {package}")


def _unquote(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] == '"':
        return value[1:-1]
    raise ReleaseError(f"expected a quoted string, got {value}")


def _xml(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


def _flag(value: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise ReleaseError(f"expected true or false, got {value}")


def _write_if_changed(path: Path, text: str) -> None:
    if path.is_file() and path.read_text() == text:
        return
    # newline="\n": Cargo.lock is LF on every platform; the default
    # translation would write CRLF on Windows and corrupt the lock.
    path.write_text(text, newline="\n")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="data_release.py")
    commands = parser.add_subparsers(dest="cmd", required=True)

    read = commands.add_parser("read-version")
    read.add_argument("cargo")

    mark = commands.add_parser("marker")
    mark.add_argument("version")

    decided = commands.add_parser("plan")
    decided.add_argument("--assets-changed", required=True)
    decided.add_argument("--version", required=True)
    decided.add_argument("--tag-exists", required=True)
    decided.add_argument("--release-exists", required=True)
    decided.add_argument("--dispatch-settled", required=True)
    decided.add_argument("--head-pending", required=True)
    decided.add_argument("--tag-is-data-release", required=True)

    applied = commands.add_parser("apply")
    applied.add_argument("--from", dest="src", required=True)
    applied.add_argument("--to", required=True)
    applied.add_argument("--date", required=True)
    applied.add_argument("--summary", required=True)
    applied.add_argument("--cargo", required=True)
    applied.add_argument("--lock", required=True)
    applied.add_argument("--metainfo", required=True)

    upcoming = commands.add_parser("next-version")
    upcoming.add_argument("--current", required=True)
    upcoming.add_argument("--subjects-file", required=True)

    compare = commands.add_parser("cmp-version")
    compare.add_argument("--current", required=True)
    compare.add_argument("--tag", required=True)

    args = parser.parse_args(argv)
    try:
        if args.cmd == "read-version":
            print(package_version(Path(args.cargo).read_text()))
        elif args.cmd == "marker":
            print(marker(args.version))
        elif args.cmd == "plan":
            print(
                plan(
                    assets_changed=_flag(args.assets_changed),
                    version=args.version,
                    tag_exists=_flag(args.tag_exists),
                    release_exists=_flag(args.release_exists),
                    dispatch_settled=_flag(args.dispatch_settled),
                    head_pending=_flag(args.head_pending),
                    tag_is_data_release=_flag(args.tag_is_data_release),
                )
            )
        elif args.cmd == "apply":
            cargo_path = Path(args.cargo)
            lock_path = Path(args.lock)
            meta_path = Path(args.metainfo)
            cargo, lock, meta = apply_tree(
                cargo_path.read_text(),
                lock_path.read_text(),
                meta_path.read_text(),
                src=args.src,
                dest=args.to,
                date=args.date,
                summary=args.summary,
            )
            _write_if_changed(cargo_path, cargo)
            _write_if_changed(lock_path, lock)
            _write_if_changed(meta_path, meta)
        elif args.cmd == "next-version":
            subjects = Path(args.subjects_file).read_text().splitlines()
            print(next_version(current=args.current, subjects=subjects))
        elif args.cmd == "cmp-version":
            print(cmp_version(current=args.current, tag=args.tag))
        else:
            parser.error(f"unknown command {args.cmd}")
    except (OSError, ReleaseError) as error:
        print(error, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
