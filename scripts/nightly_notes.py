#!/usr/bin/env python3
"""The "What changed" section of a nightly's release notes.

    scripts/nightly_notes.py <owner/repo> <since> <until>

Prints Markdown listing the commits between two full SHAs, newest first:
`since`, where the `nightly` tag stood, empty if it stands nowhere, and
`until`, the commit being published. A commit whose subject ends `(#N)`, as
GitHub's squash merges do, is listed as that pull request; any other by its
own hash, so that a commit pushed straight to `main` is not left out.

It lists from `gh api compare`, not GitHub's generated release notes: those
leave such a commit out, write `by @author` on every line, and link a
comparison of tags, when this tag moves every night and the link goes empty.
"""

import re
import subprocess
import sys

PULL_REQUEST = re.compile(r"^(.*?)\s*\(#(\d+)\)$")
CODE_SPAN = re.compile(r"(`[^`]*`)")
MENTION = re.compile(r"(?<![\w`])@[\w-]+(?:/[\w-]+)*")


def commits_between(repo, since, until):
    """`(sha, subject)`s, oldest first; `None` if GitHub would not say."""
    done = subprocess.run(
        [
            "gh",
            "api",
            "--paginate",
            f"repos/{repo}/compare/{since}...{until}?per_page=100",
            "--jq",
            r'.commits[] | "\(.sha) \(.commit.message | split("\n")[0])"',
        ],
        capture_output=True,
        text=True,
    )
    if done.returncode != 0:
        # A nightly is worth more than its list: warn, and say so in the notes.
        reason = done.stderr.strip().replace("\n", " ")
        print(f"::warning::no list of changes since {since}: {reason}", file=sys.stderr)
        return None
    return [tuple(line.partition(" ")[::2]) for line in done.stdout.splitlines()]


def plain(subject):
    """`subject` as Markdown that shows it and notifies nobody.

    Outside a code span, `<` opens an HTML tag that GitHub then removes
    (`Buffer<Fast>` renders as `Buffer`), and `@name` notifies its owner each
    time the release is made again. `\\@name` still does: only a code span stops
    it. All three were read off `gh api markdown`, 2026-11-01.
    """
    parts = CODE_SPAN.split(subject)
    for i in range(0, len(parts), 2):
        parts[i] = MENTION.sub(lambda m: f"`{m[0]}`", parts[i].replace("<", "\\<"))
    return "".join(parts)


def entry(url, sha, subject):
    merged = PULL_REQUEST.match(subject)
    if merged:
        title, number = merged.groups()
        return f"- {plain(title)} ([#{number}]({url}/pull/{number}))"
    return f"- {plain(subject)} ([{sha[:7]}]({url}/commit/{sha}))"


def section(repo, since, until, commits):
    """The section, for `commits` as `commits_between` gives them."""
    url = f"https://github.com/{repo}"
    head = "## What changed\n\n"
    if not since:
        return f"{head}No earlier nightly to compare with: see [the history of `main`]({url}/commits/{until})."
    diff = f"{url}/compare/{since}...{until}"
    if commits is None:
        return f"{head}The list could not be made for this build: see [the comparison with the last nightly]({diff})."
    if not commits:
        return f"{head}Nothing since the last nightly: this build is of the same commit."
    lines = "\n".join(entry(url, sha, subject) for sha, subject in reversed(commits))
    return f"{head}Since the last nightly ([full diff]({diff})):\n\n{lines}"


def main(argv):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    repo, since, until = argv
    commits = commits_between(repo, since, until) if since else []
    print(section(repo, since, until, commits))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
