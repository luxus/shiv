#!/usr/bin/env python3
"""Assemble and inspect model-leveled CompLock artifact metadata.

The CompLock watch publishes `pack-data.json`, `meta.json`, and `SHA256SUMS` to
the rolling `complock-data` prerelease without bumping app versions. This
script resolves `requires_shiv` from tagged `MODEL_LEVEL` constants in git
history, decides whether a publish or embedded-asset commit is needed, and
derives the options baseline file used by `complock_watch`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Iterable, Sequence

SCHEMA_VERSION = 1
DEFAULT_SOURCE = "https://deadlockconfig.com/"
COMPLOCK_RS_PATH = "crates/shiv-core/src/complock.rs"
_TAG_RE = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
_MODEL_LEVEL_RE = re.compile(r"\bpub\s+const\s+MODEL_LEVEL\s*:\s*u64\s*=\s*(\d+)\s*;")


class ArtifactError(Exception):
    pass


def read_model_level(rust_source: str) -> int:
    match = _MODEL_LEVEL_RE.search(rust_source)
    if not match:
        raise ArtifactError("MODEL_LEVEL constant not found in complock.rs")
    level = int(match.group(1))
    if level < 1:
        raise ArtifactError(f"MODEL_LEVEL must be >= 1, got {level}")
    return level


def parse_semver_tag(tag: str) -> tuple[int, int, int] | None:
    match = _TAG_RE.match(tag.strip())
    if not match:
        return None
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)))


def resolve_requires_shiv(
    target_model: int, tag_levels: Iterable[tuple[str, int]]
) -> str | None:
    if target_model < 1:
        raise ArtifactError(f"target_model must be >= 1, got {target_model}")
    candidates: list[tuple[tuple[int, int, int], str]] = []
    for tag, level in tag_levels:
        if level < target_model:
            continue
        parsed = parse_semver_tag(tag)
        if parsed is not None:
            candidates.append((parsed, tag.removeprefix("v")))
    if not candidates:
        return None
    candidates.sort(key=lambda item: item[0])
    return candidates[0][1]


def scan_git_tags_for_requires_shiv(
    repo_dir: Path, target_model: int, complock_relpath: str = COMPLOCK_RS_PATH
) -> str | None:
    if target_model < 1:
        raise ArtifactError(f"target_model must be >= 1, got {target_model}")
    listed = subprocess.run(
        ["git", "-C", str(repo_dir), "tag", "-l", "v*"],
        check=True,
        capture_output=True,
        text=True,
    )
    semver_tags: list[tuple[tuple[int, int, int], str]] = []
    for raw in listed.stdout.splitlines():
        tag = raw.strip()
        parsed = parse_semver_tag(tag)
        if parsed is not None:
            semver_tags.append((parsed, tag))
    semver_tags.sort(key=lambda item: item[0])
    for _parsed, tag in semver_tags:
        shown = subprocess.run(
            ["git", "-C", str(repo_dir), "show", f"{tag}:{complock_relpath}"],
            capture_output=True,
            text=True,
        )
        if shown.returncode != 0:
            continue
        try:
            level = read_model_level(shown.stdout)
        except ArtifactError:
            # Older release tags predated MODEL_LEVEL and cannot claim support.
            continue
        if level >= target_model:
            return tag.removeprefix("v")
    return None


def build_meta(
    *,
    model: int,
    requires_shiv: str | None,
    game: str,
    fetched: str,
    source_sha256: str,
    prev_meta: dict[str, Any] | None = None,
    schema: int = SCHEMA_VERSION,
    source: str = DEFAULT_SOURCE,
) -> dict[str, Any]:
    if not game.isdigit():
        raise ArtifactError(f"game build must be digits, got {game!r}")
    if len(source_sha256) != 64 or any(c not in "0123456789abcdef" for c in source_sha256):
        raise ArtifactError(f"invalid source_sha256: {source_sha256!r}")
    preserved_fetched = fetched
    if (
        prev_meta is not None
        and prev_meta.get("source_sha256") == source_sha256
        and isinstance(prev_meta.get("fetched"), str)
        and prev_meta["fetched"]
    ):
        preserved_fetched = prev_meta["fetched"]
    return {
        "schema": schema,
        "model": model,
        "requires_shiv": requires_shiv,
        "game": game,
        "fetched": preserved_fetched,
        "source": source,
        "source_sha256": source_sha256,
    }


def format_sha256sums(files: Sequence[tuple[str, bytes]]) -> str:
    lines = [
        f"{hashlib.sha256(content).hexdigest()}  {name}\n"
        for name, content in sorted(files, key=lambda item: item[0])
    ]
    return "".join(lines)


def meta_material_key(meta: dict[str, Any]) -> tuple[Any, ...]:
    return (
        meta.get("schema"),
        meta.get("model"),
        meta.get("requires_shiv"),
        meta.get("game"),
        meta.get("source_sha256"),
    )


def publish_needed(prev_meta: dict[str, Any] | None, new_meta: dict[str, Any]) -> bool:
    return prev_meta is None or meta_material_key(prev_meta) != meta_material_key(new_meta)


def extract_options(pack_bytes: bytes) -> str:
    data = json.loads(pack_bytes.decode("utf-8"))
    if not isinstance(data, dict):
        raise ArtifactError("pack-data must be a JSON object")
    options = {k: v for k, v in data.items() if k != "gameinfo"}
    return json.dumps(options, indent=1) + "\n"


def _load_optional_meta(path_str: str | None) -> dict[str, Any] | None:
    if not path_str:
        return None
    path = Path(path_str)
    if not path.is_file():
        return None
    data = json.loads(path.read_text(encoding="utf-8"))
    return data if isinstance(data, dict) else None


def _baseline_model(
    prev_meta: dict[str, Any] | None, assets_meta: dict[str, Any] | None
) -> int:
    for meta in (prev_meta, assets_meta):
        if meta is None:
            continue
        raw = meta.get("model")
        if isinstance(raw, int) and not isinstance(raw, bool) and raw >= 1:
            return raw
    return 1


def _parse_bool(value: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise ArtifactError(f"expected true or false, got {value!r}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="complock_artifact.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    ext = sub.add_parser("extract-options")
    ext.add_argument("pack_data")
    ext.add_argument("out_options")

    asm = sub.add_parser("assemble")
    asm.add_argument("--pack-data", required=True)
    asm.add_argument("--game", required=True)
    asm.add_argument("--fetched", required=True)
    asm.add_argument("--head-complock", required=True)
    asm.add_argument("--repo", required=True)
    asm.add_argument("--model-bumped", required=True)
    asm.add_argument("--out-dir", required=True)
    asm.add_argument("--prev-meta")
    asm.add_argument("--assets-meta")

    args = parser.parse_args(argv)
    try:
        if args.cmd == "extract-options":
            options_text = extract_options(Path(args.pack_data).read_bytes())
            Path(args.out_options).write_text(options_text, encoding="utf-8", newline="\n")
        elif args.cmd == "assemble":
            pack_bytes = Path(args.pack_data).read_bytes()
            source_sha256 = hashlib.sha256(pack_bytes).hexdigest().lower()
            prev_meta = _load_optional_meta(args.prev_meta)
            assets_meta = _load_optional_meta(args.assets_meta)
            prev_model = _baseline_model(prev_meta, assets_meta)
            model_bumped = _parse_bool(args.model_bumped)
            next_model = prev_model + 1 if model_bumped else prev_model
            head_model = read_model_level(
                Path(args.head_complock).read_text(encoding="utf-8")
            )
            requires_shiv = scan_git_tags_for_requires_shiv(Path(args.repo), next_model)
            new_meta = build_meta(
                model=next_model,
                requires_shiv=requires_shiv,
                game=args.game,
                fetched=args.fetched,
                source_sha256=source_sha256,
                prev_meta=prev_meta,
            )
            needed = publish_needed(prev_meta, new_meta)
            assets_compatible = next_model <= head_model

            out_dir = Path(args.out_dir)
            out_dir.mkdir(parents=True, exist_ok=True)
            meta_bytes = (json.dumps(new_meta, indent=2) + "\n").encode("utf-8")
            sums_bytes = format_sha256sums(
                [("meta.json", meta_bytes), ("pack-data.json", pack_bytes)]
            ).encode("utf-8")
            (out_dir / "pack-data.json").write_bytes(pack_bytes)
            (out_dir / "meta.json").write_bytes(meta_bytes)
            (out_dir / "SHA256SUMS").write_bytes(sums_bytes)

            print(f"model={next_model}")
            print(f"head_model={head_model}")
            print(f"requires_shiv={requires_shiv or ''}")
            print(f"source_sha256={source_sha256}")
            print(f"publish_needed={'true' if needed else 'false'}")
            print(f"assets_compatible={'true' if assets_compatible else 'false'}")
            print(f"fetched={new_meta['fetched']}")
        else:
            parser.error(f"unknown command {args.cmd}")
    except (ArtifactError, OSError, json.JSONDecodeError, subprocess.CalledProcessError) as err:
        print(err, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
