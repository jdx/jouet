# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2](https://github.com/jdx/jouet/compare/v0.0.1...v0.0.2) - 2026-10-31

### Added

- *(deps)* add jouet deps, which checks a package against its registry and OSV before it is added ([#90](https://github.com/jdx/jouet/pull/90))
- *(docs)* add jouet docs, a dependency's API at the locked version ([#88](https://github.com/jdx/jouet/pull/88))
- *(test)* print each failure within a budget and keep the full log, with --cmd for any command ([#87](https://github.com/jdx/jouet/pull/87))
- *(ci)* add jouet ci explain, which says whether a CI failure is flaky, infrastructure or real ([#85](https://github.com/jdx/jouet/pull/85))
- *(read)* rebuild the repository map on the code graph ([#73](https://github.com/jdx/jouet/pull/73))
- *(sessions)* add jouet sessions to show which agent sessions work where ([#67](https://github.com/jdx/jouet/pull/67))
- batch edit replace across files, run plain unittest, and spare agents five lookups ([#81](https://github.com/jdx/jouet/pull/81))
- *(claim)* add jouet claim, advisory claims between sessions on one machine ([#72](https://github.com/jdx/jouet/pull/72))
- *(edit)* scope structural rewrites and searches to a symbol's callers or impact ([#70](https://github.com/jdx/jouet/pull/70))
- *(rule)* warn on resetting a branch to its remote, and on rebasing over a merged base ([#76](https://github.com/jdx/jouet/pull/76))
- *(stack)* add jouet stack sync, restacking by merge with verified results ([#75](https://github.com/jdx/jouet/pull/75))
- *(stack)* add jouet stack status, checking each layer by ancestry and content ([#74](https://github.com/jdx/jouet/pull/74))
- *(setup)* prime sessions in four lines and install the skill only with --skill ([#71](https://github.com/jdx/jouet/pull/71))
- *(wait)* add jouet wait daemon and steer declared services to pitchfork ([#60](https://github.com/jdx/jouet/pull/60))
- *(wind)* propose mise tasks for repeated commands, and add them with jouet task apply ([#66](https://github.com/jdx/jouet/pull/66))
- *(job)* add jouet job for one-off background work, and jouet wait job ([#63](https://github.com/jdx/jouet/pull/63))
- *(edit)* structural search and rewrites with ast-grep ([#49](https://github.com/jdx/jouet/pull/49))
- *(graph)* seed a new worktree's graph from a sibling's, and prune gone ones ([#57](https://github.com/jdx/jouet/pull/57))
- *(graph)* wire the code graph into routines, edit, read and test --affected ([#55](https://github.com/jdx/jouet/pull/55))
- *(graph)* link calls, imports and inheritance, and query the graph ([#51](https://github.com/jdx/jouet/pull/51))
- *(graph)* add jouet-graph, a code graph derived from codegraph ([#44](https://github.com/jdx/jouet/pull/44))
- *(ci)* give jouet ci run the secrets its jobs read from fnox ([#59](https://github.com/jdx/jouet/pull/59))
- *(commit)* keep fnox secrets out of commits and the transcript ([#58](https://github.com/jdx/jouet/pull/58))
- *(diag)* report the diagnostics an edit introduced, through hk ([#48](https://github.com/jdx/jouet/pull/48))
- *(ci)* run GitHub Actions jobs locally through jactionlint ([#50](https://github.com/jdx/jouet/pull/50))
- *(agent-help)* group the overview by command family ([#47](https://github.com/jdx/jouet/pull/47))
- *(test)* add jouet test --affected with vitest, jest and pytest adapters ([#42](https://github.com/jdx/jouet/pull/42))
- *(read)* read and edit code by symbol ([#45](https://github.com/jdx/jouet/pull/45))
- *(search)* add budgeted search with session memory and a repo map ([#43](https://github.com/jdx/jouet/pull/43))
- *(share)* propose fixes to jouet itself from what sessions show ([#39](https://github.com/jdx/jouet/pull/39))
- *(wind)* review whole sessions through a compact digest ([#38](https://github.com/jdx/jouet/pull/38))
- *(joue)* add Luau routines on jouet's primitives, run by joue ([#37](https://github.com/jdx/jouet/pull/37))
- *(rule)* define rules in TOML, with Luau as an escape hatch ([#36](https://github.com/jdx/jouet/pull/36))
- *(bench)* add an agent benchmark harness that measures jouet with and without its hooks ([#35](https://github.com/jdx/jouet/pull/35))
- *(completion)* add shell completions and a man page from the usage spec ([#32](https://github.com/jdx/jouet/pull/32))
- *(git)* add jouet commit, review-package and handoff ([#24](https://github.com/jdx/jouet/pull/24))
- *(rule)* require fires/quiet examples on every rule ([#31](https://github.com/jdx/jouet/pull/31))
- *(stats)* add jouet stats, what jouet saved you ([#26](https://github.com/jdx/jouet/pull/26))
- *(audit)* add jouet audit, a local report on where agent sessions spend tokens and time ([#22](https://github.com/jdx/jouet/pull/22))
- *(share)* propose proven rules and missing primitives to jouet ([#25](https://github.com/jdx/jouet/pull/25))
- *(wind)* draft rules from the hook ledger, and unwind the noisy ones ([#21](https://github.com/jdx/jouet/pull/21))
- *(setup)* add agent-help, skill and harness hooks that run jouet's rules ([#15](https://github.com/jdx/jouet/pull/15))
- *(rule)* add a Luau rule engine with built-in steering rules ([#17](https://github.com/jdx/jouet/pull/17))
- *(wait)* add jouet wait ci, run and pr for GitHub ([#9](https://github.com/jdx/jouet/pull/9))
- *(pr)* add jouet pr threads ([#8](https://github.com/jdx/jouet/pull/8))
- *(status)* add jouet pr status ([#5](https://github.com/jdx/jouet/pull/5))
- *(wait)* add jouet wait for files, logs, ports, URLs and processes ([#7](https://github.com/jdx/jouet/pull/7))
- *(edit)* add jouet edit for exact, all-or-nothing edits ([#3](https://github.com/jdx/jouet/pull/3))

### Fixed

- *(rule)* count a passing jouet test --cmd of the whole suite as a full run ([#91](https://github.com/jdx/jouet/pull/91))
- *(rule)* quiet scoped-lint on the full check and large-read on logs ([#89](https://github.com/jdx/jouet/pull/89))
- *(rule)* give probes their own time, so a slow git no longer silences a rule ([#86](https://github.com/jdx/jouet/pull/86))
- *(edit)* name the count to set when an edit matches too often or too rarely ([#84](https://github.com/jdx/jouet/pull/84))
- *(handoff)* stop leaving a pitchfork supervisor running in every repository ([#82](https://github.com/jdx/jouet/pull/82))
- *(rule)* do not record a refused jouet command as a failed check ([#68](https://github.com/jdx/jouet/pull/68))
- *(threads)* keep listing threads when GitHub sends a comment without an ID ([#69](https://github.com/jdx/jouet/pull/69))
- *(edit)* say when a search holds part of a line, and quiet scoped-lint on a clean tree ([#64](https://github.com/jdx/jouet/pull/64))
- *(bench)* keep a run's refreshed login, and run a suite on one copy of jouet ([#62](https://github.com/jdx/jouet/pull/62))
- make main compile again after #38 and #39 ([#41](https://github.com/jdx/jouet/pull/41))
- *(setup)* install per-project hooks into settings.local.json, and skip them when jouet is missing ([#34](https://github.com/jdx/jouet/pull/34))
- *(verify)* a partial test run can no longer clear a failed full run ([#33](https://github.com/jdx/jouet/pull/33))

### Other

- *(task)* name mise 2026.10.25 as the release jouet task apply needs ([#80](https://github.com/jdx/jouet/pull/80))
- *(read)* drop the repo map until jouet-graph replaces it ([#52](https://github.com/jdx/jouet/pull/52))
- *(ci)* fail on unused dependencies with cargo-shear ([#54](https://github.com/jdx/jouet/pull/54))
- split jouet into workspace crates ([#19](https://github.com/jdx/jouet/pull/19))
- redraw the homepage showcase as diagrams and polish the docs ([#20](https://github.com/jdx/jouet/pull/20))
- rebrand the site and README around the robot ([#16](https://github.com/jdx/jouet/pull/16))
- add bats e2e tests and Windows-only Pester tests ([#10](https://github.com/jdx/jouet/pull/10))
- add the jouet logo ([#13](https://github.com/jdx/jouet/pull/13))
- add the documentation site at jouet.jdx.dev ([#6](https://github.com/jdx/jouet/pull/6))
- scaffold the repository ([#2](https://github.com/jdx/jouet/pull/2))

## [0.0.1](https://github.com/jdx/jouet/compare/v0.0.0...v0.0.1) - 2026-10-30

### Other

- scaffold the repository ([#2](https://github.com/jdx/jouet/pull/2))
- Initial commit
