#!/usr/bin/env python3
"""Every relative Markdown link in the repository's tracked `.md` files
resolves to a file or directory that exists. Exit 1, listing each broken
link, when one does not.

    scripts/check-links.py            # check
    scripts/check-links.py --verbose  # also count the links checked

Links to URLs and bare `#anchors` are not checked; a link's `#fragment` is
dropped before the path is resolved. Run it after moving or renaming a doc.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")


def tracked_markdown():
    out = subprocess.run(["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return [ROOT / line for line in out.splitlines() if line]


def main():
    broken, checked = [], 0
    for md in tracked_markdown():
        in_fence = False
        for number, line in enumerate(md.read_text(encoding="utf-8").splitlines(), 1):
            if line.lstrip().startswith("```"):
                in_fence = not in_fence
                continue
            if in_fence:
                continue
            # Inline code is not a link: `[](…)` is a lambda.
            for target in LINK.findall(re.sub(r"`[^`]*`", "", line)):
                if re.match(r"^[a-z]+:", target) or target.startswith("#"):
                    continue
                path = target.split("#", 1)[0]
                if not path:
                    continue
                checked += 1
                if not (md.parent / path).exists():
                    broken.append(f"{md.relative_to(ROOT)}:{number}: {target}")
    for b in broken:
        print(b)
    if "--verbose" in sys.argv or broken:
        print(f"{checked} relative links checked, {len(broken)} broken")
    sys.exit(1 if broken else 0)


if __name__ == "__main__":
    main()
