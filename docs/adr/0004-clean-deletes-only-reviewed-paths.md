# clean deletes only the paths it is given, after someone read them

`clean` first walked every Rule and deleted each Match the moment nothing held it, so space came back while the walk went on. The first full clean that way deleted the `node_modules` of programs installed under hidden directories: each passed every check, and a person or an agent reading the list would have seen `~/.pi/agent/install/releases/1.2.0/node_modules` for what it was. No check in code can know every way a Rule over-reaches; reading the paths can catch what the checks miss.

So `clean` takes the paths to delete (`--path`, repeated, or a line each on stdin) and refuses to run without them; `plan --all` lists them as it walks, the agent reads them and drops what is not a project's build or a cache, and `clean` judges each path given again as it deletes it. A path no Rule matches is not deleted, whatever it is.

Considered: keeping a bare `clean` for unattended runs, with the review as advice in the help. An agent in a hurry skips advice; the step that catches what Rules get wrong must be the only way to delete. Accepted cost: two commands where there was one, and nothing goes while the walk is still running; `clean` reads only the paths it is given, so the second step is quick.
