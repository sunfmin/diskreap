# Safety comes from confirmed Rules and a fixed floor, not a root allowlist

A delete is allowed when a person-confirmed Rule matches it and no Hold applies. The tool has no list of directories it may delete under. Instead a fixed floor in the code applies to every Rule: it never deletes `/`, `$HOME`, a git repository's root, anything git tracks, anything outside `$HOME` and the user's own `/private/var/folders` entry, anything not owned by the user, anything on another volume, or the target of a symlink. A Rule that searches any depth must name a Marker.

We first planned an allowlist of roots (`$TMPDIR`, `~/Library/Caches`, …). It could not express the most valuable Rules: build output such as `target` and `node_modules` lives anywhere under `~/Developments`, and an allowlist wide enough to reach it protects nothing. The person agreeing to each Rule (after `rule check` shows what it would match) is the judgment the allowlist was trying to replace; the floor covers what no Rule may override, however it is written.

Consequence: a careless Rule can delete regenerable files anywhere under `$HOME` that are git-ignored and older than its Minimum age. That is the accepted risk.
