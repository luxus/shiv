#!/usr/bin/env python3
"""Fail when a pak number is chosen from files alone.

file_pak_numbers counts VPK files and nothing else. used_pak_numbers was
that function's old name. A free-slot search that called it handed pak10
to a new mod, and the sidecar of a pak Kai had deleted took the new file
(fdee4cb, Improved Quickbuy). Take numbers from pak_slot::occupied and
pak_slot::free_slots. free_slots also keeps 1..=complock::last_slot() for
an installed CompLock pack (b2c5f0e).
"""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    src = root / "crates" / "shiv-core" / "src"
    if not src.is_dir():
        print(f"no crate source at {src}", file=sys.stderr)
        return 2

    problems: list[str] = []
    for path in sorted(src.rglob("*.rs")):
        for line_no, line in enumerate(path.read_text().splitlines(), 1):
            where = f"{path.relative_to(root)}:{line_no}"
            if "used_pak_numbers(" in line:
                problems.append(
                    f"{where}: used_pak_numbers counts files only. "
                    "Call pak_slot::occupied, or pak_slot::free_slots to take a number."
                )
            if "file_pak_numbers(" in line and "fn file_pak_numbers(" not in line:
                if path.name != "pak_slot.rs":
                    problems.append(
                        f"{where}: file_pak_numbers counts files only. "
                        "Call pak_slot::occupied, or pak_slot::free_slots to take a number."
                    )

    install = (src / "install.rs").read_text()
    if "pak_slot::free_slots(" not in install:
        problems.append(
            "crates/shiv-core/src/install.rs: new-mod slots come from "
            "pak_slot::free_slots. That is the hold-back for an installed "
            "CompLock pack (b2c5f0e) and the sidecar set (fdee4cb), in one place."
        )
    if "pak_slot::occupied(" not in install:
        problems.append(
            "crates/shiv-core/src/install.rs: a slot named by a sidecar is "
            "taken. Call pak_slot::occupied (fdee4cb)."
        )

    sidecar = (src / "sidecar.rs").read_text()
    if "pak_slot::occupied(" not in sidecar:
        problems.append(
            "crates/shiv-core/src/sidecar.rs: set_pak must refuse a number "
            "pak_slot::occupied holds when no file in the plan does "
            "(SidecarError::StalePak)."
        )

    if problems:
        print("pak occupancy check failed:")
        for problem in problems:
            print(problem)
        return 1
    print("pak occupancy check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
