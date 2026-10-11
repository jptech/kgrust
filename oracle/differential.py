#!/usr/bin/env python3
"""Compare kg-hir's lowering against the reference's own tree, node for node.

STATUS: paths sort (pass 8 slice 3), so the spine-collapsing normalizer this
script used to carry is gone — the two sides are compared raw. The only
remaining normalization is the reference's redundant-import warning seed, a
subtree kg-hir deliberately does not build (it exists only to carry a parser
warning, which this port reports from the module graph, kg_db::imports).

The round-trip gate cannot see a deterministic lowering bug: the same bug sits
on both sides of lower(parse(pretty(lower(parse(f))))) and cancels. This is the
only comparison whose two sides do not share our lowering.

Both dumps are pre-order. We compare the *sequence of constructor names*, which
is enough to catch a missing node, an extra node, or reordered children — the
whole symmetric-bug class — without either side having to imitate the other's
field formatting.

`cargo xtask ir` runs this same comparison natively over `library/` when the
oracle binary is built; this script remains the by-hand instrument for a single
file's diff.
"""
import glob, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
KG = os.path.join(HERE, "..", "target", "release", "kg")
# `oracle/build.sh`'s output, wherever cabal put it for this platform and GHC.
ORACLE = next(iter(glob.glob(os.path.join(HERE, "dist-newstyle", "build", "*", "*",
                                          "oracle-1.0.0", "x", "kg-oracle", "build",
                                          "kg-oracle", "kg-oracle"))), "kg-oracle")
LIB  = os.path.join(os.environ.get("KANAGAWA_ROOT",
                                   os.path.join(HERE, "..", "third_party", "kanagawa")), "library")
BASE = LIB + "/base.k"

def ours(path):
    out = subprocess.run([KG, "dump", "ir", "--no-manifest", "--base-library", BASE,
                          "-I", LIB, path], capture_output=True, text=True).stdout
    names = []
    for line in out.split("\n"):
        s = line.strip()
        if not s: continue
        # `seg:` lines are an *unsorted* Path's segments, not nodes. An
        # uppercase segment (`seg: T`) would otherwise read as a constructor.
        if s.startswith("seg:"): continue
        if s.startswith("unsorted path"): break        # the trailing report
        s = re.sub(r"^[a-z_]+:\s*", "", s)             # strip the field label
        m = re.match(r"([A-Z][A-Za-z]*)", s)
        if m: names.append(m.group(1))
    return names

def theirs(path):
    # The oracle prints one tree per parsed file, in completion order. The
    # root is *usually* last — but a root that base.k itself imports (tuple.k,
    # fifo.k, …) completes mid-list, with base.k last. Pick the root's tree by
    # its module header instead; a headerless snippet cannot be imported, so
    # for those the last line really is the root.
    out = subprocess.run([ORACLE, "-I", LIB, "--base", BASE, "--tree", path],
                         capture_output=True, text=True).stdout
    lines = [l for l in out.strip().split("\n") if l] if out.strip() else []
    if not lines: return []
    m = re.search(r"^\s*module\s+([\w.\-]+)", open(path).read(), re.M)
    line = lines[-1]
    if m:
        components = ",".join(f'"{c}"' for c in m.group(1).split("."))
        needle = f"ModuleDeclF {{declName = ModuleIdentifierF [{components}], "
        # Last match: a device file can share its module name with the one the
        # search order gave base's own import, and the root completes last.
        for l in reversed(lines):
            if needle in l:
                line = l
                break
    # `show` emits every constructor with an F suffix, in pre-order.
    names = [n[:-1] for n in re.findall(r"\b([A-Z][A-Za-z]*F)\b", line)]
    return drop_warning_seed(names)

def drop_warning_seed(names):
    # The reference leads every program with a seed `Seq` carrying any
    # redundant-import warnings (`Parser.hs:234-241`); kg-hir omits it —
    # those warnings come from the module graph (`kg_db::imports`). The seed
    # is `Warning*` wrapping one `Seq`.
    if len(names) < 2 or names[0] != "Seq": return names
    i = 1
    while i < len(names) and names[i] == "Warning": i += 1
    if i < len(names) and names[i] == "Seq":
        return names[:1] + names[i + 1:]
    return names

def main(paths):
    ok = 0
    for path in paths:
        a, b = ours(path), theirs(path)
        if not b:
            print(f"SKIP {path}  (oracle produced no tree)")
            continue
        if a == b:
            ok += 1
            print(f"OK   {path}  ({len(a)} nodes)")
        else:
            print(f"DIFF {path}  ours={len(a)} theirs={len(b)}")
            for i, (x, y) in enumerate(zip(a, b)):
                if x != y:
                    print(f"     first at #{i}: ours={x} theirs={y}")
                    print(f"     ours   ...{a[max(0,i-3):i+4]}")
                    print(f"     theirs ...{b[max(0,i-3):i+4]}")
                    break
            else:
                print(f"     prefix agrees; length differs")
                print(f"     ours   tail {a[len(b):][:6]}")
                print(f"     theirs tail {b[len(a):][:6]}")
    print(f"{ok}/{len(paths)} agree")

main(sys.argv[1:])
