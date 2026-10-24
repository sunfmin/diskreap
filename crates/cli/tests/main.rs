//! The real binary, run against a home and a temporary directory of its own,
//! with `gh`, `lsof`, `ps` and `mdfind` stood in for (examples/standin.rs).

mod common;

mod clean;
mod help;
mod rules;
mod scan;
