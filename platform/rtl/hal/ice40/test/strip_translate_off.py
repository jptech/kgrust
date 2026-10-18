"""Copy SystemVerilog sources with their simulation-only regions removed.

    strip_translate_off.py <out dir> <source>...

Generated RTL and upstream's runtime fence code synthesis must not see
between `synopsys translate_off` and `translate_on` comments (string tables,
dynamic assertions, `real` arithmetic). Synthesis tools honour the comments;
sv2v drops every comment first, so the regions are cut here. Each output is
numbered in input order, which also keeps two sources with one basename apart.

For check.zsh beside it; `kg board`'s flow does the same in Rust
(crates/kg-board/src/repair.rs, `strip_translate_off`).
"""

import re
import sys

OFF = re.compile(r"(synopsys|pragma)\s+translate_off")
ON = re.compile(r"(synopsys|pragma)\s+translate_on")

out_dir, sources = sys.argv[1], sys.argv[2:]
for n, path in enumerate(sources):
    kept, skipping = [], False
    with open(path) as f:
        for line in f:
            if OFF.search(line):
                skipping = True
            elif ON.search(line):
                skipping = False
            elif not skipping:
                kept.append(line)
    if skipping:
        sys.exit(f"{path}: translate_off without translate_on")
    with open(f"{out_dir}/{n:03d}_{path.rsplit('/', 1)[-1]}", "w") as f:
        f.writelines(kept)
