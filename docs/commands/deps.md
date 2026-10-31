---
description: Check a package against its registry and OSV before adding it, whether it exists, its latest version, licence, deprecation and advisories, and the project's locked dependencies against OSV.
---

# jouet deps

Agents add dependencies from memory. Sometimes the name does not exist, which is how hallucinated names and typosquats get installed. Sometimes the package is deprecated, the version written is years old, or a known advisory affects it. `jouet deps info` asks the package's registry and [OSV](https://osv.dev) before the dependency is written into a manifest, and `jouet deps audit` checks every version the project's lockfiles lock.

```text
jouet deps info ECOSYSTEM:NAME[@VERSION]... [--refresh]
jouet deps audit [--ecosystem cargo|npm|pypi|go] [--refresh]
```

Every flag is listed in the [CLI reference](/cli/deps).

## One package

```output
$ jouet deps info cargo:time@0.2.7
cargo time 0.2.7 (latest 0.3.55)
  licence MIT OR Apache-2.0; repository https://github.com/time-rs/time
  RUSTSEC-2020-0071 (medium): Potential segfault in the time crate; fixed in 0.2.23; also CVE-2020-26235, GHSA-wcg3-cvx6-7396
(exit 1)

$ jouet deps info cargo:serde-jsonx
cargo serde-jsonx: not on crates.io
  did you mean: serde_json?
(exit 3)

$ jouet deps info npm:request
npm request 2.88.2 (latest)
  deprecated: request has been deprecated, see https://github.com/request/request/issues/3142
  licence Apache-2.0; repository https://github.com/request/request
  GHSA-p8p7-x288-28g6 (medium): Server-Side Request Forgery in Request; no fixed version; also CVE-2023-28155
(exit 1)
```

`ECOSYSTEM` is `cargo` (crates.io), `npm`, `pypi` or `go` (the Go module proxy). Several packages can be checked in one call, `jouet deps info npm:zod npm:valibot`, and each gets its own entry.

A bare `NAME` takes its ecosystem from the nearest lockfile that locks it. When none does, the one kind of manifest in the project decides it: `Cargo.toml`, `package.json`, `pyproject.toml` or `requirements.txt`, `go.mod`. When the project has several, name the ecosystem.

The version checked is the one given, else the one the lockfile locks, else the latest. For each package it reports:

- whether it exists, and for one that does not, close names that crates.io's or npm's search finds. Only names a typo or two away with at least 10,000 recent downloads are offered, since a little-used name a typo away may itself be a typosquat. PyPI and the Go proxy have no search, so they suggest nothing;
- its latest version, and the registry's spelling when it differs (crates.io answers `serde_json` for `serde-json`, PyPI `flask-cors` for `Flask_Cors`);
- whether that version is yanked (crates.io, PyPI) or retracted (a Go module's `retract`), and whether the package is deprecated: npm's message, a Go module's `// Deprecated:` comment, or PyPI's `Development Status :: 7 - Inactive` classifier. crates.io has no deprecation;
- its licence and repository. The Go proxy has no licence;
- the OSV advisories that affect the version, each with its severity and the first later version that fixes it. Advisories several databases publish (a GHSA and a RUSTSEC entry for the same bug) are one entry, with the other IDs as `also`. OSV's malware records (`MAL-`) are marked `malicious`, and RustSec's `unmaintained` and `unsound` notices show their kind.

## The locked dependencies

```output
$ jouet deps audit
11 of 1309 locked packages have advisories (OSV; pnpm-lock.yaml 1276, uv.lock 34 (1 private, not sent))
npm fast-uri 3.1.8 (pnpm-lock.yaml)
  GHSA-w74x-v6f9-j3xg (medium): fast-uri vulnerable to URI scheme spoofing via percent-encoded scheme decoding; fixed in 3.1.9; also CVE-2026-110306
npm undici 6.29.0 (pnpm-lock.yaml)
  GHSA-2m5v-pch6-74hx (high): undici vulnerable to TLS certificate validation bypass via dropped tls options in Agent; fixed in 6.29.1; also CVE-2026-109383
# (the other nine affected packages are cut from this page)
```

It reads the nearest lockfile of each ecosystem from the current directory up to the repository's root, the same ones [`jouet docs`](/commands/docs) reads: `Cargo.lock`; `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock` or `bun.lock`; `uv.lock` or `poetry.lock`; and `go.mod`. Every version they lock goes to OSV in batched `querybatch` requests of up to 1000, and only the affected packages are listed. `--ecosystem` reads only one ecosystem's lockfile.

## What leaves the machine

Only package names and versions. A registry is asked about one package by name (and one version of it); OSV is asked about (ecosystem, name, version) triples. Packages a lockfile locks from anywhere but the public registry are never sent: a private registry or index, a path, a workspace member, a git repository, a module `replace`d in `go.mod` or matching `GOPRIVATE`. `audit` counts them as private.

Requests use jouet's TLS (the system's root certificates) and time out after 15 seconds. A dropped connection, a 5xx or a 429 is retried once; after that the command exits 5.

## Caching

Answers are cached under jouet's cache directory (`$XDG_CACHE_HOME/jouet/deps`, else `~/.cache/jouet/deps`): a registry's for ten minutes, OSV's for a day, so a second check of the same package costs no request. `--refresh` asks again and replaces the cached answers.

## JSON

`jouet deps info cargo:time@0.2.7 --json` prints:

```json
{
  "packages": [
    {
      "ecosystem": "cargo",
      "name": "time",
      "latest": "0.3.55",
      "version": "0.2.7",
      "license": "MIT OR Apache-2.0",
      "repository": "https://github.com/time-rs/time",
      "advisories": [
        {
          "id": "RUSTSEC-2020-0071",
          "aliases": ["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"],
          "summary": "Potential segfault in the time crate",
          "severity": "medium",
          "fixed": "0.2.23",
          "url": "https://osv.dev/vulnerability/RUSTSEC-2020-0071"
        }
      ]
    }
  ]
}
```

A package also has `registry_name` when the registry spells it differently, `lockfile` when the version came from one, `yanked` and `yanked_reason`, `deprecated` (its message), and `notes`. One that does not exist has `not_found` and `suggestions`; a version that does not exist has `version_not_found`. An advisory may have `informational` (`malicious`, `unmaintained`, `unsound`, `notice`). Severity is `low`, `medium`, `high` or `critical`: GitHub's rating where the advisory has one, else the rating of its CVSS 3 vector.

`jouet deps audit --json` prints `lockfiles` (each with `ecosystem`, `path`, `packages` and `private`), `checked`, the number of versions sent to OSV, and `affected`, each with `ecosystem`, `name`, `version`, `lockfile` and `advisories`.

## In a routine

[Routines](/routines) call it as `jouet.deps.info{packages}` and `jouet.deps.audit{}`, with the `net` capability. Each returns `{ ok, exit_code, json, error, command }`, `json` being the document above.

## In hooks

There is no rule that runs `jouet deps` on `cargo add`, `npm install` or `pip install`: hooks do not use the network unless `rules.network` is on, so such a rule would stay quiet for most users.

## Exit codes

| Code | When |
|---|---|
| `0` | Every package exists and the versions checked have no advisories; `audit` found none |
| `1` | A version checked has an advisory, or is yanked or retracted, or the package is deprecated; `audit` found an affected package |
| `2` | An ecosystem is not `cargo`, `npm`, `pypi` or `go`, or a bare name could be in several |
| `3` | A package, or the version asked for, does not exist; `audit` found no lockfile |
| `5` | A registry or OSV could not be reached, or answered with an error |

When `info` checks several packages, a missing one (3) outranks an advisory (1).
