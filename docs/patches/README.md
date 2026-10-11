# Patches

> **Both patches below are history (2026-09-27).** Each is against the
> twin's era — its map and its read-back seam, deleted 2026-09-26 — and no
> longer applies; what each measured is in its journal entry. New parked work
> goes here with a row and a journal entry, as before.

Work that is **written and measured but deliberately not in the tree**, kept
as a patch so the next session does not rewrite it from the journal's prose.

Each one is against the commit that added it and will rot; the journal entry
that names it is the authority on *why* it is here and what it is worth.

| patch | journal entry |
|---|---|
| `instance-notes-in-the-twins-map.patch` | "The note that fixes the types and costs the walk" (2026-09-20) — `setTypeOfInstance`'s note threaded into the map a module hands its importers. Takes `ir`'s `type agreement` 21/38 → 34/38 and `ptdiff` 1590 → 1498, because this port's `Ty::Instance` is not transparent where the reference's `TInstance` is |
| `settling-round.patch` | "The settling round, measured whole and not landed" (2026-09-20) — one extra `inferType` round that visits a body whose parameters never resolve, plus the `setTypeOfTemplateCall` stamp it must not carry across the read-back seam. Worth +3 backend agreements and +1 `ptdiff` structural; costs the worst-case keystroke 578 ms → 1313 ms |

Landed, and removed from here:

* `hoisted-lambda-scope.patch` — in the tree since 2026-09-22 with the half
  it lacked: `fixScopedName` reading the references un-aliased as `move_one`
  reads the declaration. Either half alone costs the same four
  `higher-order-functions` programs; see the journal, "A `static if` over a
  type, decided where the reference decides it".
