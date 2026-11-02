#!/usr/bin/env python3
"""Drafts a release's notes from `.github/release-template.md`. It publishes nothing.

    scripts/release_notes.py draft [--archives DIR] [--since REV] [--repo OWNER/REPO] [--force]
    scripts/release_notes.py check FILE

`draft` reads what is already written down: the version from `Cargo.toml`, the
story from `CHANGELOG.md`, the parity numbers from `docs/status.md`, the
archives from DIR (default `target/dist`) and the pull requests since the last
`v*` tag. It writes `target/release-notes/v<version>.md`, with a `TODO(release)`
where a person must write, and prints the `gh` command that makes a draft
release from it. `check` fails while a `TODO(release)` is left.

AGENTS.md, "Releasing", has the whole procedure. A person reads the draft and
publishes it: nothing in the nightly workflow makes a release.
"""

import argparse
import hashlib
import re
import shlex
import subprocess
import sys
import tarfile
from pathlib import Path

sys.dont_write_bytecode = True
import nightly_notes  # noqa: E402

MARKER = "TODO(release)"
PLATFORMS = {
    "aarch64-apple-darwin": "macOS (Apple silicon)",
    "x86_64-unknown-linux-gnu": "Linux x86_64",
    "aarch64-unknown-linux-gnu": "Linux arm64",
}
BLOCK_START = re.compile(r"\s*(?:[-*+] |\d+\. |\|)")
RELATIVE_LINK = re.compile(r"\]\((?!https?://|#)([^)\s]+)\)")


class Refused(Exception):
    """What must be put right before there can be notes."""


def package_version(cargo_toml):
    found = re.search(r'^\[workspace\.package\]$.*?^version = "([^"]+)"$', cargo_toml, re.M | re.S)
    if not found:
        raise Refused("Cargo.toml has no [workspace.package] version")
    return found[1]


def unwrap(text):
    """One line per paragraph and per bullet.

    A release's notes keep every newline, as `<br>` (`gh api markdown`,
    2026-11-01), and `CHANGELOG.md` is wrapped for reading in a file.
    """
    out, fenced = [], False

    def joins(previous):
        stripped = previous.strip()
        return stripped and not stripped.startswith(("#", "```"))

    for line in text.splitlines():
        if line.lstrip().startswith("```"):
            fenced = not fenced
            out.append(line)
        elif fenced or not line.strip() or BLOCK_START.match(line) or not out or not joins(out[-1]):
            out.append(line)
        else:
            out[-1] += " " + line.strip()
    return "\n".join(out)


def changelog_section(changelog, version, blob):
    """The section's name, its opening prose and the rest, ready for the notes.

    A link relative to the repository is made absolute at `blob`: in a release's
    notes it would otherwise resolve against the release page.
    """
    heading = re.search(rf"^## {re.escape(version)}(?: — (.*?))?(?:, \d{{4}}-\d{{2}}-\d{{2}})?$", changelog, re.M)
    if not heading:
        raise Refused(f"CHANGELOG.md has no `## {version} — name, date` section: write it first")
    rest = changelog[heading.end():]
    following = re.search(r"^## ", rest, re.M)
    section = rest[: following.start()] if following else rest
    section = RELATIVE_LINK.sub(lambda m: f"]({blob}/{m[1]})", section)
    first = re.search(r"^### ", section, re.M)
    intro, body = (section[: first.start()], section[first.start():]) if first else (section, "")
    # A blank line between two bullets would space the whole list out: GitHub
    # makes each item of such a list a paragraph.
    return heading[1], unwrap(intro.strip()), re.sub(r"^(- .*)\n\n(?=- )", r"\1\n", unwrap(body.strip()), flags=re.M)


def parity(status, url):
    """The table of what the gates last said, and the commit they said it of.

    From `docs/status.md`, which `cargo xtask status-md` renders from a green run,
    so that these are the gates' numbers and not anyone's memory of them.
    """

    def row(gate, pattern):
        cells = re.search(rf"^\| `{gate}` \| (.*?) \|", status, re.M)
        found = cells and re.search(pattern, cells[1])
        if not found:
            raise Refused(f"docs/status.md has no `{gate}` result: render it (`cargo xtask status-md`)")
        return found

    commit = re.search(r"Rendered from the gate run at `([0-9a-f]+)`", status)
    if not commit:
        raise Refused("docs/status.md does not say which run it is from: render it (`cargo xtask status-md`)")
    conformance = row("conformance", r"\*\*(\d+/\d+)\*\*")[1]
    repoint = row("repoint", r"\*\*(\d+/\d+)\*\*")[1]
    cli = row("cli", r"\*\*(\d+/\d+)\*\*")[1]
    output = row("output", r"\*\*(\d+)\*\*/(\d+) identical modulo locations.*?; (\d+) differ, (\d+) the reference failed")
    table = "\n".join(
        [
            f"Graded against the reference compiler, on its own test corpus, at [`{commit[1]}`]({url}/commit/{commit[1]}):",
            "",
            "| | |",
            "|---|---|",
            f"| Frontend verdicts | **{conformance}** decidable assertions agree |",
            f"| Upstream's test runner | **{repoint}** syntax test files pass |",
            f"| Upstream's command-line tests | **{cli}** pass with `kanagawa` |",
            f"| Generated RTL | **{output[1]} of {output[2]}** programs identical apart from source locations; "
            f"{output[3]} differ, {output[4]} the reference failed |",
        ]
    )
    return commit[1], table


def read_archive(path, version):
    """`(target, MiB, has the code generator)`, once the archive has been checked."""
    name = path.name[: -len(".tar.gz")]
    digest = path.with_name(path.name + ".sha256")
    if not digest.is_file():
        raise Refused(f"{path.name} has no .sha256 beside it")
    if digest.read_text().split()[0] != hashlib.sha256(path.read_bytes()).hexdigest():
        raise Refused(f"{path.name} does not match its .sha256")
    with tarfile.open(path) as tar:
        names = tar.getnames()
        readme = tar.extractfile(f"{name}/README.md").read().decode()
    # A plain `cargo xtask dist` also names its archive by the version, and
    # reports `-dev+<commit>`; the archive's README states what `kg` said.
    if f"version `{version}`" not in readme:
        raise Refused(f"{path.name} does not report {version}: build it with `cargo xtask dist --release`")
    backend = any(re.search(r"/bin/libkanagawa-backend\.", member) for member in names)
    return name[len(f"kgrust-{version}-"):], path.stat().st_size / 1048576, backend


def find_archives(directory, version):
    """The release's archives under `directory`, at any depth: `gh run download`
    gives each artifact a directory of its own."""
    paths = sorted(Path(directory).rglob(f"kgrust-{version}-*.tar.gz"))
    if not paths:
        raise Refused(f"no kgrust-{version}-*.tar.gz under {directory}")
    names = [path.name for path in paths]
    for name in names:
        if names.count(name) > 1:
            raise Refused(f"{name} is under {directory} more than once: keep the one to release")
    return paths


def archive_table(paths, version, url, tag):
    read = [(path, *read_archive(path, version)) for path in paths]
    order = list(PLATFORMS)
    read.sort(key=lambda a: order.index(a[1]) if a[1] in order else len(order))
    rows = ["| Platform | Download | Size | `kg build` |", "|---|---|---:|---|"]
    for path, target, size, backend in read:
        link = f"[{path.name}]({url}/releases/download/{tag}/{path.name})"
        rows.append(f"| {PLATFORMS.get(target, target)} | {link} | {size:.1f} MiB | {'yes' if backend else 'no'} |")

    notes = []
    if not all(a[3] for a in read):
        notes.append(
            "Where the table says no, the archive has no code generator: `kg check`, `fmt`, `lsp`, `doc`, `explain` "
            "and `board list`/`new` work, and `kg build` needs `libkanagawa-backend`, built from source with "
            "`scripts/build-backend.sh` and named by `KG_BACKEND_DIR`."
        )
    if any("-linux-" in a[1] for a in read):
        notes.append("Linux archives need glibc 2.35 or later (Ubuntu 22.04 and later).")
    for _, target, _, _ in read:
        if target.endswith("-apple-darwin"):
            notes.append(
                "On macOS the binaries are not signed. If a browser downloaded the archive, clear its quarantine "
                f"first: `xattr -dr com.apple.quarantine kgrust-{version}-{target}`."
            )
    notes.append("Each archive's `.sha256` is beside it.")
    return "\n".join(rows), "\n".join(f"- {note}" for note in notes)


def pull_requests(repo, since, until, tag):
    url = f"https://github.com/{repo}"
    if not since:
        return f"The first tagged release: see [the history of `main`]({url}/commits/{tag})."
    commits = nightly_notes.commits_between(repo, since, until)
    if commits is None:
        raise Refused(f"GitHub would not list the commits since {since}")
    lines = [nightly_notes.entry(url, sha, subject) for sha, subject in reversed(commits)]
    lines += ["", f"**Full diff:** [{since}...{tag}]({url}/compare/{since}...{tag})"]
    return "\n".join(lines).strip()


def fill(template, values):
    def value(placeholder):
        if placeholder[1] not in values:
            raise Refused(f"the template names `{placeholder[0]}`, which this script does not fill")
        return values[placeholder[1]]

    return re.sub(r"\{\{(\w+)\}\}", value, template)


def git(root, *args):
    done = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True)
    return done.stdout.strip() if done.returncode == 0 else None


def origin_repo(root):
    found = re.search(r"github\.com[:/]([^/]+/[^/]+?)(?:\.git)?$", git(root, "remote", "get-url", "origin") or "")
    if not found:
        raise Refused("origin is not on GitHub: name the repository with --repo")
    return found[1]


def draft(root, archives, since=None, repo=None, force=False):
    root = Path(root)
    version = package_version((root / "Cargo.toml").read_text())
    tag = f"v{version}"
    repo = repo or origin_repo(root)
    url = f"https://github.com/{repo}"
    until = git(root, "rev-parse", "HEAD")
    if since is None:
        since = git(root, "describe", "--tags", "--abbrev=0", "--match", "v[0-9]*", "--exclude", tag, "HEAD") or ""

    out = root / "target" / "release-notes" / f"{tag}.md"
    if out.exists() and not force:
        raise Refused(f"{out} is there, perhaps with your edits in it: move it, or pass --force")

    name, intro, changelog = changelog_section((root / "CHANGELOG.md").read_text(), version, f"{url}/blob/{tag}")
    graded, parity_table = parity((root / "docs" / "status.md").read_text(), url)
    paths = find_archives(archives, version)
    table, install_notes = archive_table(paths, version, url, tag)
    notes = fill(
        (root / ".github" / "release-template.md").read_text(),
        {
            "version": version,
            "intro": intro or f"{MARKER}: a sentence or two on what this release is for.",
            "archives": table,
            "install_notes": install_notes,
            "parity": parity_table,
            "changelog": changelog,
            "pull_requests": pull_requests(repo, since, until, tag),
        },
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(notes)

    title = f"kgrust {version}" + (f" — {name}" if name else "")
    files = [str(f) for path in paths for f in (path, path.with_name(path.name + ".sha256"))]
    command = ["gh", "release", "create", tag, "--draft", "--target", until, "--title", title, "--notes-file", str(out), *files]
    behind = git(root, "rev-list", "--count", f"{graded}..HEAD")
    print(f"Wrote {out}.")
    where = f"{behind} commit(s) before this one" if behind else "not in this history"
    print(f"Parity numbers: graded at {graded}, {where}.")
    print(f"\nFill in each {MARKER}, then:\n")
    print(f"  scripts/release_notes.py check {out}")
    print(f"  {shlex.join(command)}")
    print("\nThen read the draft on GitHub and publish it there, which makes the tag.")


def check(path):
    left = [
        f"{path}:{number}: {line[:80]}"
        for number, line in enumerate(Path(path).read_text().splitlines(), 1)
        if MARKER in line
    ]
    if left:
        raise Refused(f"{len(left)} {MARKER} left:\n" + "\n".join(left))
    print(f"{path}: nothing left to write.")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    making = commands.add_parser("draft", help="write the notes")
    making.add_argument("--archives", default="target/dist")
    making.add_argument("--since", help="the revision to list from; default the last v* tag")
    making.add_argument("--repo", help="OWNER/REPO; default origin's")
    making.add_argument("--force", action="store_true", help="overwrite notes already drafted")
    checking = commands.add_parser("check", help="fail while a TODO(release) is left")
    checking.add_argument("file")
    args = parser.parse_args(argv)
    try:
        if args.command == "draft":
            draft(Path(__file__).resolve().parent.parent, args.archives, args.since, args.repo, args.force)
        else:
            check(args.file)
    except Refused as refused:
        print(f"release_notes: {refused}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
