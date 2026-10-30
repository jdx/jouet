// Highlights the homepage showcase's code at build time, with the same Shiki
// setup as the docs pages, so the component ships HTML rather than a
// highlighter. Output shown here is real jouet output.
import { createMarkdownRenderer, type SiteConfig } from "vitepress";

const snippets = {
  editBefore: [
    "python",
    `for path, old, new in edits:
    s = Path(path).read_text()
    assert s.count(old) == 1, path
    Path(path).write_text(s.replace(old, new))`,
  ],
  editBeforeOutput: [
    "output",
    `Traceback (most recent call last):
  File "<stdin>", line 7, in <module>
AssertionError: Cargo.toml`,
  ],
  editAfter: [
    "spec",
    `=== src/main.rs
<<<<<<< SEARCH
    let x = 1;
=======
    let x = 2;
>>>>>>> REPLACE
=== Cargo.toml
<<<<<<< SEARCH
version = "0.1.0"
=======
version = "0.2.0"
>>>>>>> REPLACE`,
  ],
  editOutput: [
    "output",
    `not written src/main.rs  1 edit  +1 -1
failed      Cargo.toml   1 edit
  edit 1: no match
    closest at line 3 (94% similar), - spec / + file:
    -version = "0.1.0"
    +version = "0.1.1"
0 of 2 files written`,
  ],
  logBefore: [
    "sh",
    `until grep -q 'Listening on' app.log; do
  sleep 2
done`,
  ],
  logAfter: [
    "sh",
    "jouet wait log app.log --match 'Listening on' --timeout 2m",
  ],
  logOutput: [
    "output",
    `[0s] log app.log: following
[10s] log app.log: matched
MET log app.log: matched (10s)
  12:06:31 Listening on http://localhost:3000`,
  ],
  ciBefore: [
    "sh",
    `sleep 60; gh pr checks 42
# …next turn: the same again
gh run view 41083340820 --log-failed`,
  ],
  ciAfter: ["sh", "jouet wait ci 15195 --repo jdx/mise --check release"],
  ciOutput: [
    "output",
    `FAIL jdx/mise#15195 @ 843cc16  0 pass, 1 fail, 0 skipped, 0 pending  (3s)

FAIL release  step "Require auto-merge on the release PR"  required
  https://github.com/jdx/mise/actions/runs/41083340820/job/124157989724
  ##[error]Enable auto-merge on this pull request to build and test the release tarballs.
  ##[error]Process completed with exit code 1.
  full log: ~/.local/state/jouet/logs/jdx-mise-job-124157989724.log`,
  ],
} as const;

export type Showcase = Record<keyof typeof snippets, string>;

declare const data: Showcase;
export { data };

export default {
  async load(): Promise<Showcase> {
    const config = (globalThis as { VITEPRESS_CONFIG?: SiteConfig })
      .VITEPRESS_CONFIG;
    if (!config) throw new Error("showcase.data.mts runs inside VitePress");
    const md = await createMarkdownRenderer(
      config.srcDir,
      config.markdown,
      config.site.base,
      config.logger,
    );
    return Object.fromEntries(
      Object.entries(snippets).map(([key, [lang, code]]) => [
        key,
        md.render(`\`\`\`${lang}\n${code}\n\`\`\`\n`),
      ]),
    ) as Showcase;
  },
};
