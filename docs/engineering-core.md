# Reusable Engineering Core (v1)

This document is the project-agnostic rule core. Any repository can adopt it by
picking a profile (`Lean`, `Strict`, or `Exemplar`) and keeping only
project-specific invariants, gates, and bug-to-enforcer tables in its own
`AGENTS.md` / `docs/`.

## Profiles

Choose one profile per repository (or per crate tier when a repo contains
ported/vendored code):

| Area | `Lean` (scripts, MVPs, CRUD) | `Strict` (production apps & libs) | `Exemplar` (native, perf-critical, reference code) |
| --- | --- | --- | --- |
| Purpose & visibility | No dead code, minimal visibility | Compiler-enforced dead-code/visibility hygiene | Strict + zero unused `pub` exports or speculative abstractions |
| Comments | No "what" comments; sparse "why" comments | `Lean` + doc contracts on public items | `Strict` + existing comments audited and retired |
| Errors & panics | Never swallow errors | No unproved `unwrap`/`expect` in prod; honest error variants | `Strict` + type-level capabilities and rollback atomicity |
| Trust boundaries | Validate untrusted inputs | Path/URL/redirect/size caps enforced | `Strict` + explicit regression tests per boundary |
| Tests | Happy path + bug regressions | Invariant/property tests for core logic | `Strict` + structural guards, fail-closed CI gates |
| Proofs & perf gates | Only when bottlenecked | Measurements for perf/architecture decisions | Recorded baselines, CI bench diffs, memory/alloc ceilings |
| Standing PR question | What did you prove? | What did you prove? Any regressions? | What did you prove? (mandatory) Faster? Smaller? |

Do **not** apply `Exemplar` to throwaway prototypes, exploratory notebooks, or
CRUD apps whose bottlenecks are external I/O rather than CPU/RSS/binary size.

## The Six Laws

### 1. Every symbol earns its place
- Every file, type, function, and export has a concrete, current reason to exist.
- Visibility defaults to the narrowest scope (`private` -> crate-internal -> `pub`). Never widen visibility to silence dead-code warnings.
- No speculative abstractions, pass-through wrappers without invariants, or single-implementation traits without a test seam.

### 2. No "what" comments; "why" only where code and types cannot carry it
- Forbidden: inline comments that narrate what code does, section banner comments (`// ---`), commented-out code, untracked `TODO`s.
- Allowed (sparingly, 1-2 lines): inline comments only for non-obvious **why** (OS/allocator/protocol quirks, `SAFETY` invariants, ordering hazards) that cannot be expressed in a name or type.
- Doc comments (`///`) state contracts on public items (purpose, invariants, refusal reasons) and on private items **only** when an automated guard reads that contract (for example, `/// Worker body`).

### 3. Names and control flow match intent
- Queries and constructors name what they return (`free_slots`, `pak_of`); side-effecting operations carry a verb (`install_files`, `apply`, `restore_last`).
- Early returns for every refusal; avoid nesting deeper than three levels; avoid magic numbers (use enums/types for screen IDs, error codes, states).
- Functions over ~80 lines or production files over ~600-800 lines are a decomposition signal.

### 4. Validate before computing, refuse before touching, guard every trust boundary
- Range/capacity/validity checks run before overflow-prone arithmetic and before the first disk or state mutation; refused calls leave disk and state unchanged.
- No unproved `unwrap`, `expect`, or unchecked indexing on untrusted/external data in production code. Every `#[allow(...)]` is tightly scoped and carries `reason = "..."`.
- Error messages name the actual cause (never report a network socket error as a file error).
- Every trust boundary is guarded before work begins:
  - External paths (archives, manifests, JSON, symlinks): reject traversal (`..`), absolute paths, drive prefixes, and symlink escapes.
  - External URLs & redirects: validate scheme and allowed origins on **every hop** (`max_redirects(0)` + explicit hop verification); reject userinfo tricks and malformed authorities.
  - Untrusted streams/decoders: enforce hard byte/sample/time limits **before** allocating or decoding.
- No hidden cross-thread state coupling (such as reading UI-thread `thread_local!` singletons from worker threads); pass explicit snapshots/`Arc` inputs and route completions back to the UI thread through one dispatcher.
- Global/static caches either hold a bounded budget recorded in the memory plan or release when idle (`Weak` / scope-bound), verified by a retained-heap test.

### 5. Rules live in the build, not in hope
- Every standing rule names its automated enforcer (type system > compiler lint > static guard > test > CI gate).
- Prefer compile-time visibility/capability tokens over text-scanning scripts whenever the language can express the rule.
- Static guards and CI gates must be **fail-closed** (missing baselines or parse errors fail the gate, never exit green) and document their known blind spots explicitly.
- Never close a lint/cleanup pass without wiring its enforcer into build config or CI.
- Mark workspace crate tiers explicitly (`First-Party` vs `Port/Vendor`) so no crate sits in an unguarded middle ground.
- Every bug that represents a class of mistakes becomes a rule plus a regression/invariant test.

### 6. Decisions carry measurements (`Strict` / `Exemplar`)
- Performance, memory, dependency, and architecture decisions name the rejected alternatives and commit the measurement (date, machine, method, command) beside the code. Obvious choices say so in one line.
- Tests assert invariants across generated or boundary inputs rather than hand-computed examples.
- Every PR answers: *What did you prove?* (mandatory) — and under `Exemplar`: *What did you make faster? What did you make smaller?* (honest "not a target of this PR; existing gates unchanged" is valid when proof is a correctness/security gate).
