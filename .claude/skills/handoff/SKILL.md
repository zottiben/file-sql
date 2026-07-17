---
name: toolbox-handoff
description: Checkpoint the build before you clear/compact the context or end a session, so a fresh context resumes exactly where you left off. Commits + pushes in-progress work, certifies the repo is green with the project's own gates, and refreshes a committed HANDOFF.md "resume here" doc. Use when the context is getting full, before /clear or /compact, when wrapping up a session, or when the user says "hand off" / "clear context" / "checkpoint" / "pick this up later".
---

# toolbox handoff - checkpoint before clearing context

Make it safe to clear or compact mid-build and pick up seamlessly in a fresh context.
This *complements* the harness's native compaction - it adds the durability compaction
skips. Two things must be true when you finish: the **repo** is the durable source of
truth (committed + pushed + green), and a committed **`HANDOFF.md`** tells the next
context where things are and what to do first. A fresh context sees only what's in git
plus what it's pointed at - everything else is lost. Run the steps in order.

## 1. Commit and push in-progress work
`git status --short`. Uncommitted work vanishes on handoff. If there's any:
- Bring it to a coherent state - never leave the default branch broken; if on the
  default branch, branch first.
- Commit (Conventional Commits; never add an agent co-author) and `git push`. If
  genuinely mid-slice, commit a clearly-labelled WIP and flag it in `HANDOFF.md` (step 3).
Don't open a PR or push a `v*` tag unless the user asked.

## 2. Certify the green baseline
Give the next context a known-good point to trust. Discover and run THIS project's real
gates the way `toolbox-pre-pr` does (AGENTS.md "Commands", `package.json` scripts,
Makefile/Taskfile, CI) - typecheck / lint / test / build. Record the short HEAD sha and
each PASS/FAIL. If anything is red, fix it - or record precisely what's red and why.
Never certify green over a failure.

## 3. Refresh HANDOFF.md (the resume doc)
Write/update a committed `HANDOFF.md` at the repo root so it matches reality. Keep it
lean - a "you are here + how to resume + gotchas" pointer, not a changelog (git log,
`CHANGELOG.md`, any roadmap hold the full history). Three parts:
- **RESUME HERE** (top): branch + HEAD sha, clean/pushed status, whether a PR exists,
  and the **next 1-3 concrete work items**.
- **Gotchas learned this session** - API quirks, verification tricks, env/dep facts the
  code alone doesn't reveal. The highest-value part. A gotcha that's a *durable* project
  rule belongs in `AGENTS.md` instead (use `toolbox-capture`); `HANDOFF.md` holds only
  volatile, this-build state - trim stale detail rather than appending forever.
- **How to resume** - the one line a fresh context should start with (see step 4).
If the project auto-loads a memory (Claude Code) or a global `AGENTS.md` (Codex), mirror
the one-line RESUME pointer there too, so it surfaces without being asked.

## 4. Confirm, then it's safe to clear
Tell the user briefly: the HEAD sha, that it's pushed + green, that `HANDOFF.md` is
current, and how to resume - start a fresh session and say **"read HANDOFF.md and
continue"**. Then it's safe to `/clear` or `/compact`.

## What resume looks like (next context)
Fresh session: `git pull`, read `HANDOFF.md` (+ `AGENTS.md`), re-run the gates to confirm
the certified-green baseline still holds, then continue the next item in small, verified,
committed increments.
