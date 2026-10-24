# macOS only, and the decisions in pure crates checked by Flux

diskreap is released for macOS alone. Its Rules are about macOS's places
(`~/Library`, the per-user `/var/folders` entry, `.app` bundles) and it asks
macOS's own tools (`lsof`, `ps`, `plutil`, `mdfind`, `df`). The agent-cli
convention of releasing for Linux and Windows too is set aside until a Rule
needs another system; `--help` says so.

What decides whether a path is deleted lives in two pure crates, `rules`
(the Rule list, globs and the floor) and `judge` (the Holds and the Verdict),
which do no I/O and are checked by Flux with `0 trusted; 0 ignored`. Flux
proves that a Verdict to delete comes only from a list of Holds that is
empty, that a Minimum age is never zero nor past ten years, and that the wait
left is never zero; `#[should_fail]` functions in each crate show each of
these refuses the misuse. The effect crate reads the world into Facts and
deletes only on that Verdict.
