#!/usr/bin/env python3
"""Criterion bench-diff reporter and regression gate.

Reads the two arms of a same-runner A/B bench run: the base arm measured
the comparison-base commit with `--save-baseline push`, the candidate
arm measured the pushed commit with `--baseline-lenient push`. Writes a
markdown table of every bench's base and pushed point estimates and
criterion's change verdict to stdout and $GITHUB_STEP_SUMMARY. Exits
nonzero when a bench criterion flags as "Performance has regressed."
moved beyond REGRESSION_THRESHOLD_PCT.
"""

import argparse
import os
import re
import sys

REGRESSION_THRESHOLD_PCT = 25.0

ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
ABSOLUTE_TIME_LINE = re.compile(r"^\s*(?:(\S+)\s+)?time:\s+\[([^]]+)\]$")
THROUGHPUT_LINE = re.compile(r"^\s*thrpt:\s+\[([^]]+)\]$")
CHANGE_LINE = re.compile(r"^\s*(?:change:|time:)\s*\[([^]]*%[^]]*)\]\s*\(p\s*=")
VERDICT_LINE = re.compile(
    r"^\s+(Performance has regressed\.|Performance has improved\.|"
    r"Change within noise threshold\.|No change in performance detected\.)$"
)
STANDALONE_NAME_LINE = re.compile(r"^\S+$")

REGRESSED_VERDICT = "Performance has regressed."


def visible(line):
    return ANSI_ESCAPE.sub("", line.split("\r")[-1]).rstrip()


def midpoint(bracket):
    fields = bracket.split()
    if "%" in bracket:
        if len(fields) != 3:
            return None
        return fields[1]
    if len(fields) != 6:
        return None
    return " ".join(fields[2:4])


def parse_bench_log(paths):
    benches = {}
    pending_name = None
    current = None
    for path in paths:
        with open(path, encoding="utf-8", errors="replace") as log:
            for raw_line in log:
                line = visible(raw_line)
                absolute = ABSOLUTE_TIME_LINE.match(line)
                if absolute and "%" not in absolute.group(2):
                    name = absolute.group(1) or pending_name
                    if name:
                        benches.setdefault(
                            name, {"time": None, "thrpt": None, "change": None, "verdict": None}
                        )
                        benches[name]["time"] = midpoint(absolute.group(2))
                        current = name
                    pending_name = None
                    continue
                change = CHANGE_LINE.match(line)
                if change and current:
                    point = midpoint(change.group(1))
                    if point:
                        benches[current]["change"] = float(point.rstrip("%").replace("−", "-"))
                    continue
                throughput = THROUGHPUT_LINE.match(line)
                if throughput and current and "%" not in throughput.group(1):
                    benches[current]["thrpt"] = midpoint(throughput.group(1))
                    continue
                verdict = VERDICT_LINE.match(line)
                if verdict and current:
                    benches[current]["verdict"] = verdict.group(1)
                    continue
                if STANDALONE_NAME_LINE.match(line):
                    pending_name = line
    return benches


def cell_with_throughput(record):
    if not record:
        return "—"
    if record["thrpt"]:
        return f"{record['time']} · {record['thrpt']}"
    return record["time"]


def render_table(base, candidate, base_ref, head_ref):
    lines = [
        f"## Bench diff — `{base_ref}` → `{head_ref}`",
        "",
        "| Bench | Base | Push | Change | Criterion |",
        "|---|---|---|---|---|",
    ]
    for name, record in candidate.items():
        base_record = base.get(name)
        change = "new" if record["change"] is None else f"{record['change']:+.2f}%"
        verdict = record["verdict"] or "—"
        lines.append(
            f"| {name} | {cell_with_throughput(base_record)} | "
            f"{cell_with_throughput(record)} | {change} | {verdict} |"
        )
    lines.append("")
    lines.append(
        f"Same-runner A/B (both arms on this job's runner, back to back). "
        f"The gate fails only a bench criterion flags as regressed beyond "
        f"+{REGRESSION_THRESHOLD_PCT:.0f}%; every other movement reports."
    )
    return "\n".join(lines)


def regressed_benches(candidate):
    return sorted(
        name
        for name, record in candidate.items()
        if record["verdict"] == REGRESSED_VERDICT
        and record["change"] is not None
        and record["change"] > REGRESSION_THRESHOLD_PCT
    )


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", nargs="+", required=True)
    parser.add_argument("--candidate", nargs="+", required=True)
    parser.add_argument("--base-ref", default="base")
    parser.add_argument("--head-ref", default="head")
    return parser.parse_args()


def main():
    args = parse_args()
    base = parse_bench_log(args.base)
    candidate = parse_bench_log(args.candidate)
    if not candidate:
        print("bench_diff_summary: no bench records in the candidate logs", file=sys.stderr)
        return 2
    report = render_table(base, candidate, args.base_ref, args.head_ref)
    print(report)
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with open(summary_path, "a", encoding="utf-8") as summary:
            summary.write(report + "\n")
    failures = regressed_benches(candidate)
    if failures:
        listing = ", ".join(f"{name} ({candidate[name]['change']:+.2f}%)" for name in failures)
        print(f"bench gate failed: {listing}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
