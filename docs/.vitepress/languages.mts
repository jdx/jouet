// Shiki grammars for the two kinds of text jouet's docs show most: the specs
// `jouet edit` reads, and the reports jouet prints. Each scope is picked for
// the colour it gets in VitePress's github-light and github-dark themes, not
// for what the name means: `entity.name.tag` is green, `keyword` red,
// `variable` amber, `constant` blue and `comment` grey.
import type { LanguageInput, ShikiTransformer } from "shiki";

const ok = "entity.name.tag.jouet-ok";
const bad = "keyword.other.jouet-bad";
const held = "variable.jouet-held";
const quiet = "comment.jouet-quiet";
const path = "entity.name.jouet-path";

const command = {
  // `$ jouet …`, continued over lines that end in a backslash.
  begin: "^(\\$)\\s+([\\w-]+)",
  beginCaptures: { 1: { name: quiet }, 2: { name: "entity.name.function" } },
  end: "(?<!\\\\)$",
  patterns: [
    { match: "'[^']*'|\"[^\"]*\"", name: "string.quoted" },
    { match: "(?<=\\s)--?[\\w-]+", name: "constant.other.flag" },
    { match: "<<|\\|", name: "keyword.operator" },
  ],
};

export const jouetSpec: LanguageInput = {
  name: "spec",
  scopeName: "source.jouet-spec",
  patterns: [
    {
      match: "^(jouet)\\s+(edit)\\b(.*?)(<<)('?\\w+'?)\\s*$",
      captures: {
        1: { name: "entity.name.function" },
        2: { name: "entity.name.function" },
        4: { name: "keyword.operator" },
        5: { name: "string.unquoted.heredoc" },
      },
    },
    { match: "^JOUET_EDIT$", name: "string.unquoted.heredoc" },
    {
      match: "^(===)\\s+(.+)$",
      captures: { 1: { name: quiet }, 2: { name: path } },
    },
    {
      match: "^(<{7,})\\s+(SEARCH)\\b(.*)$",
      captures: {
        1: { name: quiet },
        2: { name: "keyword.control" },
        3: { name: "constant.other.count" },
      },
    },
    { match: "^={7,}$", name: quiet },
    {
      match: "^(>{7,})\\s+(REPLACE)\\s*$",
      captures: { 1: { name: quiet }, 2: { name: "keyword.control" } },
    },
    {
      begin: "^\\*\\*\\* Begin Patch$",
      beginCaptures: { 0: { name: "keyword.control" } },
      end: "^\\*\\*\\* End Patch$",
      endCaptures: { 0: { name: "keyword.control" } },
      patterns: [
        {
          match: "^(\\*\\*\\* (?:Add|Delete|Update) File:|\\*\\*\\* Move to:)\\s*(.*)$",
          captures: { 1: { name: "keyword.control" }, 2: { name: path } },
        },
        { match: "^\\*\\*\\* End of File$", name: "keyword.control" },
        { match: "^@@.*$", name: "meta.diff.range" },
        { match: "^\\+.*$", name: "markup.inserted" },
        { match: "^-.*$", name: "markup.deleted" },
      ],
    },
  ],
};

export const jouetOutput: LanguageInput = {
  name: "output",
  scopeName: "source.jouet-output",
  patterns: [
    command,
    // Unified diffs (--diff) and the closest-match diff under a failed edit.
    { match: "^(---|\\+\\+\\+) .*$", name: "meta.diff.header" },
    { match: "^@@ .* @@.*$", name: "meta.diff.range" },
    { match: "^\\s*\\+.*$", name: "markup.inserted" },
    { match: "^\\s*-(?!-).*$", name: "markup.deleted" },
    // The status word each line of a report starts with.
    {
      match: "^(ok|written|created|deleted|moved|MET|PASS)(?=\\s)",
      name: ok,
    },
    {
      match: "^(failed|FAIL|FAILED|TIMEOUT|CONFLICT|NO_CHECKS|jouet:)(?=\\s)",
      name: bad,
    },
    { match: "^(not written|reverted|PENDING)(?=\\s)", name: held },
    { match: "^\\s+(FAIL|fail)(?=\\s)", name: bad },
    { match: "^\\s*##\\[error\\].*$", name: bad },
    { match: "^\\w+Error:.*$", name: bad },
    { match: "^\\s*(edit \\d+:)\\s+(.*)$", captures: { 2: { name: bad } } },
    { match: "\\bCONFLICT\\b", name: bad },
    // Progress lines and pointers to the rest.
    { match: "^\\[\\d+[smh]\\]", name: quiet },
    { match: "^\\s*(full log:|\\.\\.\\.).*$", name: quiet },
    { match: "^\\s+every edited file was restored$", name: quiet },
    { match: "^\\d+ of \\d+ files? written$", name: "markup.bold" },
    { match: "^(#|\\[)\\d+\\]?", name: "constant.numeric" },
    { match: "https?://\\S+", name: "string.other.link" },
  ],
};

export const languages = [jouetSpec, jouetOutput];

// Search text in a spec, and lines a diff removes, get a red wash; replacement
// text and added lines a green one. Done by line rather than by token so the
// wash spans the code block's width.
export const diffLines: ShikiTransformer = {
  name: "jouet:diff-lines",
  line(node, line) {
    const lang = this.options.lang;
    if (lang !== "spec" && lang !== "output") return;
    const kinds = (this.meta.jouetLines ??= lineKinds(lang, this.source));
    const kind = kinds[line - 1];
    if (kind) this.addClassToHast(node, `jouet-${kind}`);
  },
};

type Kind = "remove" | "add" | undefined;

function lineKinds(lang: string, source: string): Kind[] {
  const lines = source.split("\n");
  if (lang === "output") {
    return lines.map((text) =>
      /^(---|\+\+\+) /.test(text)
        ? undefined
        : /^\s*\+/.test(text)
          ? "add"
          : /^\s*-(?!-)/.test(text)
            ? "remove"
            : undefined,
    );
  }
  const kinds: Kind[] = [];
  let state: "out" | "search" | "replace" | "patch" = "out";
  let fence = 7;
  for (const text of lines) {
    let kind: Kind;
    if (state === "out") {
      const open = text.match(/^(<{7,})\s+SEARCH\b/);
      if (open) {
        fence = open[1].length;
        state = "search";
      } else if (text === "*** Begin Patch") {
        state = "patch";
      }
    } else if (state === "search") {
      if (text === "=".repeat(fence)) state = "replace";
      else kind = "remove";
    } else if (state === "replace") {
      if (new RegExp(`^>{${fence}}\\s+REPLACE\\s*$`).test(text)) state = "out";
      else kind = "add";
    } else if (text === "*** End Patch") {
      state = "out";
    } else if (text.startsWith("+")) {
      kind = "add";
    } else if (text.startsWith("-")) {
      kind = "remove";
    }
    kinds.push(kind);
  }
  return kinds;
}
