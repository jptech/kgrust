#!/usr/bin/env python3
"""The tests of `nightly_notes.py`; `scripts/ci.sh lint` runs them.

    python3 -B scripts/nightly_notes_test.py

`gh` is a stand-in on `PATH`, so nothing here reaches GitHub. What it asks
GitHub for is exercised by the workflow's `plan` job, on the pull request that
changes either file.
"""

import contextlib
import io
import os
import stat
import tempfile
import unittest

import nightly_notes as notes

REPO = "o/r"
URL = "https://github.com/o/r"
SINCE = "a" * 40
UNTIL = "b" * 40


def listed(*commits):
    """The lines of the list for `commits`, oldest first."""
    text = notes.section(REPO, SINCE, UNTIL, list(commits))
    return [line for line in text.splitlines() if line.startswith("- ")]


class Entries(unittest.TestCase):
    def test_a_squash_merge_is_its_pull_request(self):
        self.assertEqual(
            listed(("c" * 40, "Assorted fixes (#18)")),
            [f"- Assorted fixes ([#18]({URL}/pull/18))"],
        )

    def test_a_commit_pushed_straight_to_main_is_not_left_out(self):
        self.assertEqual(
            listed(("c" * 40, "docs: a note")),
            [f"- docs: a note ([ccccccc]({URL}/commit/{'c' * 40}))"],
        )

    def test_only_the_trailing_number_is_the_pull_request(self):
        self.assertEqual(
            listed(("c" * 40, 'Revert "Fix (#3)" (#20)')),
            [f'- Revert "Fix (#3)" ([#20]({URL}/pull/20))'],
        )

    def test_a_number_elsewhere_in_the_subject_is_not_one(self):
        self.assertEqual(
            listed(("c" * 40, "Fix (#3) again")),
            [f"- Fix (#3) again ([ccccccc]({URL}/commit/{'c' * 40}))"],
        )

    def test_newest_first(self):
        self.assertEqual(
            listed(("c" * 40, "older (#1)"), ("d" * 40, "newer (#2)")),
            [f"- newer ([#2]({URL}/pull/2))", f"- older ([#1]({URL}/pull/1))"],
        )


class Plain(unittest.TestCase):
    def test_an_angle_bracket_is_kept_from_opening_a_tag(self):
        self.assertEqual(notes.plain("Buffer<Fast> named"), "Buffer\\<Fast> named")

    def test_a_name_is_kept_from_notifying_its_owner(self):
        self.assertEqual(notes.plain("from @octocat, thanks"), "from `@octocat`, thanks")
        self.assertEqual(notes.plain("bump @types/node"), "bump `@types/node`")

    def test_an_address_is_no_mention(self):
        self.assertEqual(notes.plain("mail a@b.example"), "mail a@b.example")

    def test_a_code_span_is_left_as_written(self):
        self.assertEqual(notes.plain("`Buffer<Fast>` and `@x`"), "`Buffer<Fast>` and `@x`")

    def test_text_after_a_code_span_is_still_escaped(self):
        self.assertEqual(notes.plain("`a` then B<C> @d"), "`a` then B\\<C> `@d`")

    def test_a_pull_requests_title_is_escaped_too(self):
        self.assertEqual(
            listed(("c" * 40, "Named FIFO<Size> (#4)")),
            [f"- Named FIFO\\<Size> ([#4]({URL}/pull/4))"],
        )

    def test_a_commits_subject_is_escaped_too(self):
        self.assertEqual(
            listed(("c" * 40, "Named FIFO<Size>")),
            [f"- Named FIFO\\<Size> ([ccccccc]({URL}/commit/{'c' * 40}))"],
        )


class Section(unittest.TestCase):
    def test_a_list_links_the_whole_range(self):
        text = notes.section(REPO, SINCE, UNTIL, [("c" * 40, "x (#1)")])
        self.assertIn(f"[full diff]({URL}/compare/{SINCE}...{UNTIL})", text)

    def test_no_earlier_nightly_links_the_history_not_an_empty_range(self):
        text = notes.section(REPO, "", UNTIL, [])
        self.assertIn("No earlier nightly", text)
        self.assertIn(f"{URL}/commits/{UNTIL}", text)
        self.assertNotIn("/compare/", text)

    def test_the_same_commit_says_so(self):
        text = notes.section(REPO, SINCE, SINCE, [])
        self.assertIn("Nothing since the last nightly", text)

    def test_no_answer_does_not_read_as_no_change(self):
        text = notes.section(REPO, SINCE, UNTIL, None)
        self.assertNotIn("Nothing since", text)
        self.assertIn("could not be made", text)
        self.assertIn(f"{URL}/compare/{SINCE}...{UNTIL}", text)


class Gh(unittest.TestCase):
    """The script as the workflow runs it, with `gh` stood in for."""

    def run_with(self, gh_body, *argv):
        with tempfile.TemporaryDirectory() as tmp:
            gh = os.path.join(tmp, "gh")
            self.args_file = os.path.join(tmp, "args")
            with open(gh, "w") as f:
                f.write(f'#!/bin/sh\nprintf "%s\\n" "$@" > "{self.args_file}"\n{gh_body}\n')
            os.chmod(gh, os.stat(gh).st_mode | stat.S_IEXEC)
            path = os.environ["PATH"]
            os.environ["PATH"] = f"{tmp}{os.pathsep}{path}"
            out, err = io.StringIO(), io.StringIO()
            try:
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                    rc = notes.main(list(argv))
                self.asked = None
                if os.path.exists(self.args_file):
                    with open(self.args_file) as f:
                        self.asked = f.read().splitlines()
            finally:
                os.environ["PATH"] = path
            return rc, out.getvalue(), err.getvalue()

    def test_github_oldest_first_comes_out_newest_first(self):
        rc, out, _ = self.run_with(
            f"printf '%s\\n' '{'c' * 40} older (#1)' '{'d' * 40} a subject with  spaces'",
            REPO, SINCE, UNTIL,
        )
        self.assertEqual(rc, 0)
        self.assertLess(out.index("a subject with  spaces"), out.index("older"))

    def test_a_commit_with_no_message_is_still_listed(self):
        rc, out, _ = self.run_with(f"printf '%s\\n' '{'c' * 40} '", REPO, SINCE, UNTIL)
        self.assertEqual(rc, 0)
        self.assertIn(f"[ccccccc]({URL}/commit/{'c' * 40})", out)

    def test_it_asks_for_every_page_of_the_range(self):
        self.run_with("exit 0", REPO, SINCE, UNTIL)
        self.assertIn("--paginate", self.asked)
        self.assertIn(f"repos/{REPO}/compare/{SINCE}...{UNTIL}?per_page=100", self.asked)

    def test_a_failure_warns_and_still_gives_notes(self):
        rc, out, err = self.run_with("echo 'HTTP 502' >&2; exit 1", REPO, SINCE, UNTIL)
        self.assertEqual(rc, 0)
        self.assertIn("could not be made", out)
        self.assertIn("::warning::", err)
        self.assertIn("HTTP 502", err)

    def test_no_earlier_nightly_asks_github_nothing(self):
        rc, out, _ = self.run_with("exit 1", REPO, "", UNTIL)
        self.assertEqual(rc, 0)
        self.assertIsNone(self.asked)
        self.assertIn("No earlier nightly", out)


if __name__ == "__main__":
    unittest.main()
