# Security policy

## Reporting a vulnerability

Please do not open a public issue or discussion for a suspected vulnerability.

Use [GitHub's private vulnerability reporting][report] instead.

Include the jouet version (`jouet --version`), your operating system, and the steps to
reproduce. Remove GitHub tokens and private repository names from logs and attachments.

## Supported versions

Security fixes are made on the latest released version and on the `main` branch. Please upgrade
to the newest release before reporting an issue that may already have been fixed.

## What is in scope

jouet's commands are designed to read a GitHub token (from `JOUET_GITHUB_TOKEN`, `GH_TOKEN`,
`GITHUB_TOKEN` or `gh auth token`) and to edit files in the working tree when asked to.
Reports are especially valuable for token disclosure (in output, logs or error messages), edits
that write outside the files they name or leave a file partially written, and parsing bugs in
data received from GitHub.

[report]: https://github.com/jdx/jouet/security/advisories/new
