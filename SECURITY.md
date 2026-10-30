# Security policy

## Reporting a vulnerability

Please do not open a public issue or discussion for a suspected vulnerability.

Use the repository's **Security** tab on GitHub and choose **Report a vulnerability** to send a
private report to the maintainers. If that option is not available, open an issue that says only
that you have a security report and asks for a private way to send it. Leave the details out of
the issue.

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
