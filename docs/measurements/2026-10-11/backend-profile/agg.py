"""Aggregate a macOS `sample` call graph: inclusive samples per function
(counted once per path, at its outermost occurrence), per thread and in all,
plus the self-time list. usage: python3 agg.py file.sample.txt [top]"""
import re, sys, collections

path = sys.argv[1]
top = int(sys.argv[2]) if len(sys.argv) > 2 else 40
lines = open(path, errors="replace").read().split("\n")
start = next(i for i, l in enumerate(lines) if l.startswith("Call graph:"))
end = next(i for i, l in enumerate(lines) if l.startswith("Total number in stack") or l.startswith("Sort by top of stack"))
row = re.compile(r"^(?P<lead>[ +!:|]*)(?P<count>\d+) (?P<name>.+?)(  \(in (?P<lib>[^)]+)\))?( .*)?$")

def short(name):
    name = re.sub(r"\(.*", "", name)           # drop parameter lists
    name = re.sub(r"std::__1::", "std::", name)
    return name[:110]

incl_all = collections.Counter()
threads = []
cur = None
stack = []
for l in lines[start + 1:end]:
    m = row.match(l)
    if not m:
        continue
    depth = len(m.group("lead"))
    count = int(m.group("count"))
    name = m.group("name")
    if name.startswith("Thread_"):
        cur = {"name": name[:60], "total": count, "incl": collections.Counter()}
        threads.append(cur)
        stack = [(depth, "THREAD")]
        continue
    while stack and stack[-1][0] >= depth:
        stack.pop()
    s = short(name)
    lib = m.group("lib") or ""
    key = s + ("" if "kanagawa" in lib else f"  [{lib}]")
    if key not in (n for _, n in stack):
        cur["incl"][key] += count
        incl_all[key] += count
    stack.append((depth, key))

total = sum(t["total"] for t in threads)
print(f"{path.split('/')[-1]}: {len(threads)} threads, {total} samples in all")
for t in sorted(threads, key=lambda t: -t["total"])[:6]:
    print(f"  {t['total']:>7}  {t['name']}")
print(f"\ninclusive, all threads (top {top}):")
for k, v in incl_all.most_common(top):
    print(f"  {v:>7} {100*v/total:5.1f}%  {k}")
