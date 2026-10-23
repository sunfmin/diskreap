# Glossary

**Rule**: one kind of thing that may be deleted, written as a file glob plus the conditions under which a match may go, and why it is safe. A Rule exists only after a person agreed to it.

**Rule list**: every Rule, kept in one private gist. The only place Rules live; every run reads it again.

**Match**: a path a Rule's glob finds on this machine.

**Hold**: a reason a Match is not deleted this run — too recent, in use, an app it needs is running, its app is still installed, not the user's own. A Match with any Hold stays.

**Plan**: what a run would delete and what it would hold, with sizes, changing nothing.

**Clean**: a run that deletes every Match with no Hold.

**Scan**: a look for large directories no Rule covers, reported as candidates for new Rules; it deletes nothing and adds no Rule.

**Marker**: a file that must sit beside a Match for it to count, proving what made it — `Cargo.toml` beside `target`, `package.json` beside `node_modules`. A Rule that searches any depth needs one.

**Minimum age**: how long nothing anywhere inside a Match may have been changed or used before it may go. Set per Rule.
