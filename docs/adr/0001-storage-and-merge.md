# ADR 0001: SQLite, a mutex, and a custom diff3

The plan suggested measuring redb and fjall against SQLite. This repository uses SQLite (WAL, `BEGIN IMMEDIATE`) behind one process mutex. That is one writer, not a group-commit actor. It is enough for a single-node server and it kept the compare-and-swap test small.

`diffy::merge` treats adjacent line edits as one conflict. Landing uses a line diff built on `similar` so edits to different lines combine and edits to the same line conflict. Unified diffs still use `diffy::create_patch`.

Changeset bodies are stored inline, compressed with zstd. Spill-to-blob for huge changesets is not implemented.
