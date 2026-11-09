# Issue tracker: GitHub

Issues and specs for this repo live in GitHub Issues. Use `gh` from this checkout.

## Conventions

- Create an issue: `gh issue create --title "..." --body "..."`
- Read an issue: `gh issue view <number>`
- List issues: `gh issue list --state open`
- Make a child issue: use GitHub sub-issues; otherwise add `Part of #<parent>` to its body.
- Record blockers with native issue dependencies; otherwise use `Blocked by: #<n>`.
- Comment or manage labels with `gh issue comment` and `gh issue edit`.
- Before the first issue write in a session, confirm the target with `gh repo view --json nameWithOwner`.
- File issues only in `lee090626/synthetix`, never in `mattpocock/skills`.

## Pull requests as a triage surface

No. Treat GitHub Issues as the request surface; do not triage external PRs as feature requests.
