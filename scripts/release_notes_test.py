#!/usr/bin/env python3
"""The tests of `release_notes.py` and the template; `scripts/ci.sh lint` runs them.

    python3 -B scripts/release_notes_test.py

`gh` is a stand-in on `PATH`, so nothing here reaches GitHub, and whole drafts
are made in a temporary git repository with the real template.
"""

import contextlib
import hashlib
import io
import os
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

import release_notes as rn

REPO_ROOT = Path(__file__).resolve().parent.parent
URL = "https://github.com/o/r"
BLOB = f"{URL}/blob/v0.3.0"

CHANGELOG = """# Changelog

## [Unreleased]

### Added

- Not this one.

## 0.3.0 — better names, 2026-12-01

The headline, wrapped
over two lines. See [`docs/x.md`](docs/x.md).

### Added

- A feature, wrapped
  over two lines ([site](https://example.com/a), [here](#added)).
- Another.

## 0.2.0 — older, 2026-10-31

### Added

- Older still.
"""

STATUS = """# Status

Rendered from the gate run at `abc1234`.

| Gate | Value | What it grades |
|---|---|---|
| `conformance` | **2681/2681** decidable, 241 undecidable | accept/reject |
| `conformance --strict` | **0** failing | ratchet |
| `output` | **269**/274 identical modulo locations, 26 byte-identical; 0 differ, 5 the reference failed; locations 1 | RTL |
| `repoint` | **72/72** | runner |
| `cli` | **25/25** | CLI |
"""


def make_archive(directory, target, version="0.3.0", reports=None, backend=False, digest=True):
    """An archive as `cargo xtask dist` lays one out; returns its path."""
    name = f"kgrust-{version}-{target}"
    path = Path(directory) / f"{name}.tar.gz"
    path.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(path, "w:gz") as tar:
        members = {
            f"{name}/README.md": f"# {name}\n\nThe toolchain, version `{reports or version}`.\n".encode(),
            f"{name}/bin/kg": b"kg",
        }
        if backend:
            members[f"{name}/bin/libkanagawa-backend.dylib"] = b"lib"
        for member, data in members.items():
            info = tarfile.TarInfo(member)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    if digest:
        sha = hashlib.sha256(path.read_bytes()).hexdigest()
        path.with_name(path.name + ".sha256").write_text(f"{sha}  {path.name}\n")
    return path


class Unwrap(unittest.TestCase):
    def test_a_paragraph_becomes_one_line(self):
        self.assertEqual(rn.unwrap("one\ntwo\nthree"), "one two three")

    def test_a_bullet_takes_its_indented_lines(self):
        self.assertEqual(rn.unwrap("- one\n  two\n- three"), "- one two\n- three")

    def test_a_line_opening_with_an_inline_tag_is_still_the_same_bullet(self):
        self.assertEqual(rn.unwrap("- press\n  <kbd>F5</kbd> now"), "- press <kbd>F5</kbd> now")

    def test_a_blank_line_stays_between_paragraphs(self):
        self.assertEqual(rn.unwrap("one\n\ntwo"), "one\n\ntwo")

    def test_a_heading_takes_nothing_from_the_next_line(self):
        self.assertEqual(rn.unwrap("### Added\ntext"), "### Added\ntext")

    def test_code_is_left_as_written(self):
        code = "```sh\na\nb\n```\nafter"
        self.assertEqual(rn.unwrap(code), code)


class Changelog(unittest.TestCase):
    def section(self, version="0.3.0", text=CHANGELOG):
        return rn.changelog_section(text, version, BLOB)

    def test_the_name_is_the_headings(self):
        self.assertEqual(self.section()[0], "better names")

    def test_a_heading_with_no_name_or_date_will_do(self):
        name, intro, _ = self.section(text="## 0.3.0\n\nProse.\n")
        self.assertIsNone(name)
        self.assertEqual(intro, "Prose.")

    def test_the_prose_is_split_from_the_sections_and_unwrapped(self):
        _, intro, body = self.section()
        self.assertTrue(intro.startswith("The headline, wrapped over two lines."))
        self.assertTrue(body.startswith("### Added"))
        self.assertIn("- A feature, wrapped over two lines", body)

    def test_a_list_split_by_a_blank_line_is_one_list(self):
        body = self.section(text="## 0.3.0\n\n### Added\n\n- one\n  more\n\n- two\n\n### Fixed\n\n- three\n")[2]
        self.assertEqual(body, "### Added\n\n- one more\n- two\n\n### Fixed\n\n- three")

    def test_it_stops_at_the_next_version(self):
        self.assertNotIn("Older still", self.section()[2])
        self.assertNotIn("Not this one", self.section()[2])

    def test_a_longer_version_is_another_version(self):
        with self.assertRaisesRegex(rn.Refused, "no `## 0.3.0"):
            self.section(text="## 0.3.01 — x, 2026-12-01\n")

    def test_a_dot_in_the_version_is_a_dot(self):
        with self.assertRaisesRegex(rn.Refused, "no `## 0.3.0"):
            self.section(text="## 0a3b0 — x, 2026-12-01\n")

    def test_no_section_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "write it first"):
            self.section("9.9.9")

    def test_a_relative_link_is_made_absolute_at_the_tag(self):
        self.assertIn(f"[`docs/x.md`]({BLOB}/docs/x.md)", self.section()[1])

    def test_a_full_url_and_an_anchor_are_left_alone(self):
        body = self.section()[2]
        self.assertIn("[site](https://example.com/a)", body)
        self.assertIn("[here](#added)", body)


class Parity(unittest.TestCase):
    def test_the_numbers_are_the_gates(self):
        commit, table = rn.parity(STATUS, URL)
        self.assertEqual(commit, "abc1234")
        self.assertIn(f"[`abc1234`]({URL}/commit/abc1234)", table)
        self.assertIn("**2681/2681** decidable", table)
        self.assertIn("**72/72** syntax", table)
        self.assertIn("**25/25** pass", table)
        self.assertIn("**269 of 274** programs identical apart from source locations; 0 differ, 5 the reference failed", table)

    def test_a_missing_gate_is_refused(self):
        without = "\n".join(line for line in STATUS.splitlines() if "`repoint`" not in line)
        with self.assertRaisesRegex(rn.Refused, "`repoint`"):
            rn.parity(without, URL)

    def test_a_gate_in_another_form_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "`cli`"):
            rn.parity(STATUS.replace("**25/25**", "25 of 25"), URL)

    def test_a_table_not_rendered_from_a_run_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "which run"):
            rn.parity(STATUS.replace("Rendered from the gate run", "From a run"), URL)


class Archives(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.dir = temporary.name

    def table(self):
        return rn.archive_table(rn.find_archives(self.dir, "0.3.0"), "0.3.0", URL, "v0.3.0")

    def test_platforms_in_a_fixed_order_with_their_downloads(self):
        make_archive(self.dir, "aarch64-unknown-linux-gnu")
        make_archive(self.dir, "x86_64-unknown-linux-gnu")
        make_archive(self.dir, "aarch64-apple-darwin", backend=True)
        rows = self.table()[0].splitlines()[2:]
        self.assertTrue(rows[0].startswith("| macOS (Apple silicon) | [kgrust-0.3.0-aarch64-apple-darwin.tar.gz]"))
        self.assertTrue(rows[1].startswith("| Linux x86_64 |"))
        self.assertTrue(rows[2].startswith("| Linux arm64 |"))
        self.assertIn(f"({URL}/releases/download/v0.3.0/kgrust-0.3.0-aarch64-apple-darwin.tar.gz)", rows[0])

    def test_an_unknown_platform_is_listed_by_its_target_after_the_rest(self):
        make_archive(self.dir, "riscv64-unknown-linux-gnu")
        make_archive(self.dir, "x86_64-unknown-linux-gnu")
        rows = self.table()[0].splitlines()[2:]
        self.assertTrue(rows[0].startswith("| Linux x86_64 |"))
        self.assertTrue(rows[1].startswith("| riscv64-unknown-linux-gnu |"))

    def test_the_table_says_which_have_the_code_generator(self):
        make_archive(self.dir, "aarch64-apple-darwin", backend=True)
        make_archive(self.dir, "x86_64-unknown-linux-gnu")
        rows, notes = self.table()
        self.assertRegex(rows.splitlines()[2], r"\| yes \|$")
        self.assertRegex(rows.splitlines()[3], r"\| no \|$")
        self.assertIn("no code generator", notes)

    def test_no_note_about_the_code_generator_when_all_have_it(self):
        make_archive(self.dir, "aarch64-apple-darwin", backend=True)
        self.assertNotIn("no code generator", self.table()[1])

    def test_each_platforms_note_comes_only_with_its_archive(self):
        make_archive(self.dir, "x86_64-unknown-linux-gnu")
        notes = self.table()[1]
        self.assertIn("glibc", notes)
        self.assertNotIn("quarantine", notes)

    def test_the_macos_note_names_the_directory_to_clear(self):
        make_archive(self.dir, "aarch64-apple-darwin")
        notes = self.table()[1]
        self.assertIn("com.apple.quarantine kgrust-0.3.0-aarch64-apple-darwin`", notes)
        self.assertNotIn("glibc", notes)

    def test_an_archive_reporting_another_version_is_refused(self):
        make_archive(self.dir, "aarch64-apple-darwin", reports="0.3.0-dev+abc1234")
        with self.assertRaisesRegex(rn.Refused, r"does not report 0\.3\.0.*--release"):
            self.table()

    def test_an_archive_with_no_digest_is_refused(self):
        make_archive(self.dir, "aarch64-apple-darwin", digest=False)
        with self.assertRaisesRegex(rn.Refused, "no .sha256"):
            self.table()

    def test_an_archive_that_has_changed_since_its_digest_is_refused(self):
        path = make_archive(self.dir, "aarch64-apple-darwin")
        with open(path, "ab") as f:
            f.write(b"\0")
        with self.assertRaisesRegex(rn.Refused, "does not match"):
            self.table()

    def test_no_archive_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "no kgrust-0.3.0"):
            self.table()

    def test_another_versions_archive_is_not_this_ones(self):
        make_archive(self.dir, "aarch64-apple-darwin", version="0.2.0")
        with self.assertRaisesRegex(rn.Refused, "no kgrust-0.3.0"):
            self.table()

    def test_an_artifacts_own_directory_is_searched(self):
        make_archive(f"{self.dir}/kgrust-0.3.0-x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu")
        self.assertIn("Linux x86_64", self.table()[0])

    def test_the_same_archive_twice_is_refused(self):
        make_archive(f"{self.dir}/one", "aarch64-apple-darwin")
        make_archive(f"{self.dir}/two", "aarch64-apple-darwin", backend=True)
        with self.assertRaisesRegex(rn.Refused, "more than once"):
            self.table()


class Fill(unittest.TestCase):
    def test_a_value_is_not_read_again_as_a_placeholder(self):
        self.assertEqual(rn.fill("{{a}}", {"a": "{{b}}"}), "{{b}}")

    def test_a_placeholder_nothing_fills_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "does not fill"):
            rn.fill("{{unknown}}", {})


class Draft(unittest.TestCase):
    """A whole draft, in a repository of its own, with the real template."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "docs").mkdir()
        (self.root / ".github").mkdir()
        (self.root / "Cargo.toml").write_text('[workspace.package]\nedition = "2021"\nversion = "0.3.0"\n')
        (self.root / "CHANGELOG.md").write_text(CHANGELOG)
        (self.root / "docs" / "status.md").write_text(STATUS)
        shutil.copy(REPO_ROOT / ".github" / "release-template.md", self.root / ".github")
        self.git("init", "-q")
        self.git("add", ".")
        self.git("commit", "-qm", "first")
        self.archives = self.root / "dist"
        make_archive(self.archives, "aarch64-apple-darwin", backend=True)
        make_archive(self.archives, "x86_64-unknown-linux-gnu")
        self.notes = self.root / "target" / "release-notes" / "v0.3.0.md"

        self.bin = self.root / "stand-in"
        self.bin.mkdir()
        self.asked = self.root / "asked"
        self.gh(f"printf '%s\\n' '{'c' * 40} The change (#7)'")
        path = os.environ["PATH"]
        os.environ["PATH"] = f"{self.bin}{os.pathsep}{path}"
        self.addCleanup(os.environ.__setitem__, "PATH", path)

    def git(self, *args):
        identity = ["-c", "user.name=t", "-c", "user.email=t@example.invalid"]
        subprocess.run(["git", *identity, *args], cwd=self.root, check=True, capture_output=True)

    def gh(self, body):
        gh = self.bin / "gh"
        gh.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{self.asked}"\n{body}\n')
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)

    def draft(self, **kw):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            rn.draft(self.root, self.archives, repo="o/r", **kw)
        return out.getvalue()

    def test_every_placeholder_is_filled_and_the_parts_are_there(self):
        self.git("tag", "v0.2.0")
        self.draft()
        notes = self.notes.read_text()
        self.assertNotIn("{{", notes)
        self.assertTrue(notes.startswith("The headline, wrapped over two lines."))
        self.assertIn(f"[`docs/x.md`]({URL}/blob/v0.3.0/docs/x.md)", notes)
        self.assertIn("| macOS (Apple silicon) |", notes)
        self.assertIn("**2681/2681**", notes)
        self.assertIn("kgrust-0.3.0-<target>.tar.gz", notes)
        self.assertIn(f"{URL}/blob/v0.3.0/README.md#parity-and-speed", notes)
        self.assertIn(f"{URL}/blob/v0.3.0/docs/known-divergences.md", notes)
        self.assertIn("- A feature, wrapped over two lines", notes)
        self.assertIn(f"- The change ([#7]({URL}/pull/7))", notes)

    def test_what_a_person_must_write_is_marked(self):
        self.draft()
        self.assertIn("TODO(release)", self.notes.read_text())
        with self.assertRaisesRegex(rn.Refused, r"2 TODO\(release\) left"):
            rn.check(self.notes)

    def test_notes_with_nothing_left_to_write_pass(self):
        self.draft()
        kept = [line for line in self.notes.read_text().splitlines() if "TODO(release)" not in line]
        self.notes.write_text("\n".join(kept))
        with contextlib.redirect_stdout(io.StringIO()):
            rn.check(self.notes)

    def test_no_prose_in_the_changelog_is_left_to_a_person(self):
        (self.root / "CHANGELOG.md").write_text("## 0.3.0 — x, 2026-12-01\n\n### Added\n\n- A thing.\n")
        self.draft()
        self.assertTrue(self.notes.read_text().startswith("TODO(release)"))

    def test_the_list_starts_at_the_last_tag_and_links_it(self):
        self.git("tag", "v0.2.0")
        self.draft()
        self.assertIn("compare/v0.2.0...", self.asked.read_text())
        self.assertIn(f"[v0.2.0...v0.3.0]({URL}/compare/v0.2.0...v0.3.0)", self.notes.read_text())

    def test_its_own_tag_is_not_its_starting_point(self):
        self.git("tag", "v0.2.0")
        self.git("commit", "-qm", "later", "--allow-empty")
        self.git("tag", "v0.3.0")
        self.draft()
        self.assertIn("compare/v0.2.0...", self.asked.read_text())

    def test_the_nightly_tag_is_not_a_release(self):
        self.git("tag", "nightly")
        self.draft()
        self.assertIn("The first tagged release", self.notes.read_text())

    def test_since_names_the_starting_point(self):
        self.git("tag", "v0.2.0")
        self.draft(since="v0.1.0")
        self.assertIn("compare/v0.1.0...", self.asked.read_text())

    def test_the_newest_pull_request_comes_first(self):
        self.git("tag", "v0.2.0")
        self.gh(f"printf '%s\\n' '{'c' * 40} Older (#1)' '{'d' * 40} Newer (#2)'")
        self.draft()
        notes = self.notes.read_text()
        self.assertLess(notes.index("Newer"), notes.index("Older"))

    def test_the_first_release_asks_github_nothing(self):
        self.draft()
        self.assertFalse(self.asked.exists())
        self.assertIn(f"[the history of `main`]({URL}/commits/v0.3.0)", self.notes.read_text())

    def test_no_answer_from_github_writes_no_notes(self):
        self.git("tag", "v0.2.0")
        self.gh("echo 'HTTP 502' >&2; exit 1")
        with self.assertRaisesRegex(rn.Refused, "would not list"):
            self.draft()
        self.assertFalse(self.notes.exists())

    def test_edits_are_not_overwritten(self):
        self.draft()
        self.notes.write_text("my edits")
        with self.assertRaisesRegex(rn.Refused, "--force"):
            self.draft()
        self.assertEqual(self.notes.read_text(), "my edits")

    def test_force_overwrites(self):
        self.draft()
        self.notes.write_text("my edits")
        self.draft(force=True)
        self.assertNotIn("my edits", self.notes.read_text())

    def test_it_prints_a_command_for_a_draft_and_runs_nothing_of_the_kind(self):
        self.git("tag", "v0.2.0")
        printed = self.draft()
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=self.root, capture_output=True, text=True).stdout.strip()
        self.assertIn(f"scripts/release_notes.py check {self.notes}", printed)
        self.assertIn("gh release create v0.3.0 --draft", printed)
        self.assertIn(f"--target {head}", printed)
        self.assertIn("--title 'kgrust 0.3.0 — better names'", printed)
        self.assertIn(f"--notes-file {self.notes}", printed)
        self.assertIn("kgrust-0.3.0-aarch64-apple-darwin.tar.gz ", printed)
        self.assertIn("kgrust-0.3.0-aarch64-apple-darwin.tar.gz.sha256", printed)
        self.assertNotIn("release", self.asked.read_text())

    def test_it_says_how_old_the_parity_numbers_are(self):
        first = subprocess.run(["git", "rev-parse", "--short=7", "HEAD"], cwd=self.root, capture_output=True, text=True).stdout.strip()
        (self.root / "docs" / "status.md").write_text(STATUS.replace("abc1234", first))
        self.git("commit", "-qm", "later", "--allow-empty")
        self.assertIn(f"graded at {first}, 1 commit(s) before", self.draft())

    def test_it_says_when_the_parity_numbers_are_from_elsewhere(self):
        self.assertIn("graded at abc1234, not in this history", self.draft())

    def test_a_section_with_no_name_titles_the_release_by_version_alone(self):
        (self.root / "CHANGELOG.md").write_text("## 0.3.0\n\nProse.\n")
        self.assertIn("--title 'kgrust 0.3.0' ", self.draft())

    def test_the_repository_is_origins(self):
        for origin in ["git@github.com:o/r.git", "https://github.com/o/r.git", "https://github.com/o/r"]:
            self.git("remote", "add", "origin", origin)
            self.assertEqual(rn.origin_repo(self.root), "o/r")
            self.git("remote", "remove", "origin")

    def test_an_origin_elsewhere_is_refused(self):
        self.git("remote", "add", "origin", "https://example.com/o/r.git")
        with self.assertRaisesRegex(rn.Refused, "--repo"):
            rn.origin_repo(self.root)

    def test_no_origin_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "--repo"):
            rn.origin_repo(self.root)


class CommandLine(unittest.TestCase):
    def run_check(self, text):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "notes.md"
            path.write_text(text)
            out, err = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                rc = rn.main(["check", str(path)])
            return rc, err.getvalue()

    def test_notes_with_something_left_fail(self):
        rc, err = self.run_check("TODO(release): write this\n")
        self.assertEqual(rc, 1)
        self.assertIn(":1: TODO(release): write this", err)

    def test_finished_notes_pass(self):
        self.assertEqual(self.run_check("Finished.\n")[0], 0)


class Repository(unittest.TestCase):
    def test_the_version_is_the_workspaces(self):
        text = '[package]\nversion = "9.9.9"\n\n[workspace.package]\nversion = "0.3.0"\n'
        self.assertEqual(rn.package_version(text), "0.3.0")

    def test_no_version_is_refused(self):
        with self.assertRaisesRegex(rn.Refused, "no \\[workspace.package\\]"):
            rn.package_version('[package]\nversion = "9.9.9"\n')

    def test_this_repositorys_files_are_as_the_script_reads_them(self):
        version = rn.package_version((REPO_ROOT / "Cargo.toml").read_text())
        self.assertRegex(version, r"^\d+\.\d+\.\d+$")
        rn.parity((REPO_ROOT / "docs" / "status.md").read_text(), URL)
        changelog = (REPO_ROOT / "CHANGELOG.md").read_text()
        versions = re.findall(r"^## (\d+\.\d+\.\d+)", changelog, re.M)
        self.assertTrue(versions)
        for each in versions:
            self.assertTrue(rn.changelog_section(changelog, each, BLOB)[0], each)


if __name__ == "__main__":
    unittest.main()
