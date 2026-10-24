# afkconvoy config

How the afkconvoy fleet works this repo's backlog: issues labelled
`ready-for-agent`, worked by the team
[sunfmin/product-team](https://github.com/sunfmin/product-team), one worker
each, landed on `main` behind `./gate.sh`, the same gate a person runs.

```yaml
team: sunfmin/product-team
base_branch: "main"
gate_command: ./gate.sh
# Flux runs alone before the tests; the limit leaves room for a cold cargo
# build and a cold Flux run.
gate_seconds: 3600
ready_label: ready-for-agent
concurrency: 3
```
