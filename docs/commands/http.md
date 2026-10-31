---
description: Send an HTTP request and print the status and only the JSONPaths asked for, cut to a budget, with hurl-style assertions and credentials redacted.
---

# jouet http

Agents call APIs with `curl` and read the answer with `jq` or a Python one-liner. When the request fails, the pipe hides it: `curl -s https://api.github.com/repos/jdx/no-such-repo | jq .stargazers_count` prints `null` and exits 0. Without the pipe, the whole body lands in the context. `jouet http` sends the request, prints the status, and prints only the values at the [JSONPaths](https://www.rfc-editor.org/rfc/rfc9535) asked for. A body printed whole is cut to a byte budget, and `--expect` checks values the way [hurl](https://hurl.dev) does, exiting 1 when one is wrong.

```text
jouet http METHOD URL [-H NAME:VALUE]... [--json-body JSON | --data DATA]
                      [--select JSONPATH]... [--expect ASSERTION]...
                      [--budget BYTES] [--headers] [--no-follow] [--timeout DUR]
```

Every flag is listed in the [CLI reference](/cli/http).

## Only the values asked for

```output
$ jouet http GET https://api.github.com/repos/jdx/mise -s '$.stargazers_count' -s '$.license.spdx_id' -s '$.open_issues_count'
HTTP 200 OK, application/json; charset=utf-8, 5198 bytes, 119 ms
$.stargazers_count: 35588
$.license.spdx_id: "MIT"
$.open_issues_count: 8
```

A path that matches several values lists them, as many as fit `--budget` (4000 bytes by default; `0` removes the limit), and names the file the whole body is saved in:

```output
$ jouet http GET https://api.github.com/repos/jdx/mise/releases/latest -s '$.tag_name' -s '$.assets[*].name' --budget 300
HTTP 200 OK, application/json; charset=utf-8, 91524 bytes, 190 ms
$.tag_name: "v2026.10.25"
$.assets[*].name: 55 matches
  "install.ps1"
  "install.ps1.minisig"
  "install.ps1.sig"
  "install.sh"
  "install.sh.minisig"
  "install.sh.sig"
  "mise-v2026.10.25-linux-arm64"
  "mise-v2026.10.25-linux-arm64-musl"
  "mise-v2026.10.25-linux-arm64-musl.tar.gz"
  "mise-v2026.10.25-linux-arm64-musl.tar.xz"
  "mise-v2026.10.25-linux-arm64.tar.gz"
  ... 44 more matches (full body: ~/.local/state/jouet/logs/http-1793460668431-2466073.json)
```

`--select` takes RFC 9535 JSONPath: `$.a.b`, `$.items[0]`, `$.items[-1]`, `$.items[*].id`, `$..id`, `$.items[0:3]`, and filters such as `$.items[?@.state == 'open'].number`. A path that matches nothing prints `no match`.

Without `--select`, the body is printed, cut to the budget. JSON is printed compact for agents and indented for people (`--display human`), and the saved copy is indented so it can be read by line.

## The status, and the headers worth acting on

The first line is the status, the content type, the body's size and the time taken. Below it come only the headers an agent acts on: `location`, `retry-after`, `www-authenticate`, and the rate limit's remaining count and reset once fewer than 10 requests are left (or on a 429). `--headers` prints every header; `--json` always includes them all.

Without a `status` assertion, a status of 400 or more fails with exit 1, and the body is printed as well as any selected values, since an error page says why:

```output
$ jouet http GET https://api.github.com/repos/jdx/no-such-repo -s '$.stargazers_count'
HTTP 404 Not Found, application/json; charset=utf-8, 118 bytes, 136 ms
{"message":"Not Found","documentation_url":"https://docs.github.com/rest/repos/repos#get-a-repository","status":"404"}
$.stargazers_count: no match
expect failed: status < 400: got 404; give --expect status=404 to accept it
(exit 1)
```

Redirects are followed (up to 10, dropping `Authorization` on the way) unless `--no-follow` is given. A URL without a scheme is `http://` for `localhost`, `127.0.0.1`, `[::1]` and `*.localhost`, and `https://` for anywhere else.

## Assertions

`--expect` is repeatable, and every assertion must hold:

```output
$ jouet http GET localhost:18766/health --expect status=200 --expect 'jsonpath $.db == "ok"'
HTTP 200 OK, application/octet-stream, 33 bytes, 1 ms
{"status":"degraded","db":"down"}
expect failed: jsonpath $.db == "ok": got "down"
(exit 1)
```

An assertion is a query, then an optional `count` and an optional `not`, then a predicate and its value:

| Query | What it reads |
|---|---|
| `status` | the status code |
| `header NAME` | a response header; a header sent more than once passes if any value does |
| `jsonpath PATH` | the value at the path, or the list of values when it matches several; quote a path with spaces, as in `jsonpath "$.items[?@.state == 'open']" count >= 1` |
| `body` | the body as text |
| `duration` | the time taken, in milliseconds |

The predicates are `==` (or `=`), `!=`, `>`, `>=`, `<`, `<=`, `contains` (a substring, or an element of a list), `startsWith`, `endsWith`, `matches` (a regex), `exists` and `isEmpty`. With no predicate, the assertion is `exists`. `count` counts a list's items, or the values a path or header matched. A value is JSON when it parses as JSON (`true`, `3`, `"a b"`, `null`), and a bare string otherwise.

```text
status == 201
header Content-Type contains json
jsonpath $.items count == 3
jsonpath $.items[*].id contains 42
jsonpath $.error not exists
body matches "^ok"
duration < 500
```

The body is read as JSON whatever its content type says. The same assertions work in [`jouet wait http`](/commands/wait), which waits until they all hold: `jouet wait http localhost:3000/health --expect 'jsonpath $.ready == true'`.

## Sending a body

`--json-body` sends JSON and sets `Content-Type: application/json` unless a header gives one. It refuses text that is not JSON. `--data` sends its text as it is. Both read a file with `@FILE`, and `--data @-` reads stdin. (`--json` is jouet's flag for printing one JSON document, as on every command.)

```sh
jouet http POST https://api.example.com/items --json-body '{"name":"x"}' --expect status=201 -s '$.id'
jouet http PUT https://api.example.com/items/7 --json-body @item.json
```

## Credentials

jouet never prints a credential:

- the values of `Authorization`, `Proxy-Authorization`, `Cookie` and `Set-Cookie`, and of any header named like a token, password, API key or session, are replaced with `[redacted]`;
- so is a string under a JSON key named like one (`access_token`, `password`), in the body, selected values and assertion results;
- and so is every value jouet expands from `${NAME}`, the value of a header sent as a credential wherever the server echoes it, and, in a project that uses [fnox](https://fnox.jdx.dev), the value of each of its secrets that is in the environment;
- the text also goes through the patterns `jouet share` uses for tokens (GitHub, GitLab, Slack, AWS, OpenAI, Anthropic and others).

Give credentials by name, never written into the command. jouet expands `${NAME}` from its own environment in the URL, the headers and an inline body, so a single-quoted reference stays out of the shell's history and the transcript. In a project that keeps them in fnox, run the request under `fnox exec`:

```sh
fnox exec -- jouet http GET https://api.example.com/me -H 'Authorization: Bearer ${API_TOKEN}'
```

A `${NAME}` that is not set refuses the request with exit 4, before anything is sent.

## Exit codes

| Exit | When |
|---|---|
| 0 | the response came back and every assertion held |
| 1 | an assertion failed, or the status was 400 or more without a `status` assertion |
| 2 | a bad method, URL, header, JSONPath or assertion, or `--json-body` that is not JSON |
| 3 | a `--data` or `--json-body` file does not exist |
| 4 | a `${NAME}` is not set |
| 5 | the server could not be reached, TLS failed, or the body is larger than 64 MiB |
| 124 | no response within `--timeout` (default 30s) |

## JSON

`--json` prints one document: `method`, `url` (as given, before `${NAME}` was expanded), `status`, `reason`, `elapsed_ms`, `content_type`, `body_bytes`, `headers`, then either `body` (the parsed JSON, when nothing was selected and it fits the budget) or `body_text` with `omitted_bytes` and `body_file`, `selected` (each `path` with its `values` and `omitted`), and `assertions` (each `assertion`, `passed`, `actual` and `reason`).

## In routines

`jouet.http.request{method, url, header, json_body, data, select, expect, budget, timeout}` runs it with the `net` capability and returns its JSON document. It sends whatever method it is given, so `net` lets a routine `POST`.

## Steering

The `curl-json` rule warns on `curl` piped into `jq`, or into Python code that parses JSON (`python3 -c 'import json...'`, `python3 -m json.tool`, `python3 - <<EOF`), and points at `jouet http --select`. It stays quiet for downloads to a file (`curl -o`, `-O`), for installers (`curl ... | sh`), and for curl on its own.
