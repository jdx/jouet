import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitepress";
import { diffLines, languages } from "./languages.mts";
import { socialCard, writeSocialCard } from "./social-images.mjs";

const configDir = dirname(fileURLToPath(import.meta.url));
const cargoToml = readFileSync(resolve(configDir, "../../Cargo.toml"), "utf8");
const versionMatch = cargoToml.match(
  /^\[package\][\s\S]*?^\s*version\s*=\s*"([^"]+)"/m,
);
if (!versionMatch) {
  throw new Error("could not read the jouet version from Cargo.toml");
}
const latestVersion = versionMatch[1];
const siteUrl = "https://jouet.jdx.dev";
export default defineConfig({
  title: "jouet",
  description:
    "Wind-up tools for coding agents: one command for each shell chore agents keep scripting by hand.",
  lang: "en-US",
  lastUpdated: true,
  // Dark by default, like the rest of jdx's docs, with a light theme a click away.
  appearance: "dark",
  cleanUrls: true,
  rewrites: {
    "cli/edit.md": "cli/edit/index.md",
    "cli/pr.md": "cli/pr/index.md",
    "cli/wait.md": "cli/wait/index.md",
    "cli/rule.md": "cli/rule/index.md",
    "cli/graph.md": "cli/graph/index.md",
    // `jouet graph index` would otherwise take the group page's place.
    "cli/graph/index.md": "cli/graph/index/index.md",
  },
  sitemap: {
    hostname: siteUrl,
  },
  markdown: {
    // `spec` and `output` fences: jouet edit specs and jouet's reports.
    languages,
    codeTransformers: [diffLines],
  },
  transformPageData(pageData) {
    // The generated CLI reference gets its own styles in custom.css.
    if (pageData.relativePath.startsWith("cli/")) {
      pageData.frontmatter.pageClass = "jouet-cli-page";
    }
  },
  themeConfig: {
    logo: "/logo.svg",
    nav: [
      {
        text: "Guide",
        link: "/getting-started",
        activeMatch: "^/(getting-started|why|conventions|rules|winding|setup)",
      },
      { text: "Commands", link: "/commands/edit", activeMatch: "^/commands/" },
      { text: "Reference", link: "/cli/", activeMatch: "^/cli/" },
      {
        text: `v${latestVersion}`,
        link: "https://github.com/jdx/jouet/releases",
      },
    ],
    sidebar: [
      {
        text: "Start here",
        items: [
          { text: "Getting started", link: "/getting-started" },
          { text: "Why jouet", link: "/why" },
          { text: "Shared conventions", link: "/conventions" },
          { text: "Rules", link: "/rules" },
          { text: "Routines and joue", link: "/routines" },
          { text: "Self-winding", link: "/winding" },
          { text: "Agent setup and hooks", link: "/setup" },
          { text: "Benchmarks", link: "/benchmarks" },
        ],
      },
      {
        text: "Commands",
        items: [
          { text: "jouet edit", link: "/commands/edit" },
          { text: "jouet diag", link: "/commands/diag" },
          { text: "jouet search", link: "/commands/search" },
          { text: "jouet read", link: "/commands/read" },
          { text: "jouet graph", link: "/commands/graph" },
          { text: "jouet wait", link: "/commands/wait" },
          { text: "jouet ci run", link: "/commands/ci" },
          { text: "jouet pr status", link: "/commands/pr-status" },
          { text: "jouet pr threads", link: "/commands/pr-threads" },
          { text: "jouet pr next", link: "/commands/pr-next" },
          { text: "jouet pr land", link: "/commands/pr-land" },
          { text: "jouet test", link: "/commands/test" },
          { text: "jouet audit", link: "/commands/audit" },
          { text: "jouet stats", link: "/commands/stats" },
          { text: "jouet commit", link: "/commands/commit" },
          { text: "jouet review-package", link: "/commands/review-package" },
          { text: "jouet handoff", link: "/commands/handoff" },
        ],
      },
      {
        text: "Reference",
        items: [
          {
            text: "CLI commands",
            link: "/cli/",
            collapsed: true,
            items: [
              { text: "audit", link: "/cli/audit" },
              { text: "stats", link: "/cli/stats" },
              { text: "commit", link: "/cli/commit" },
              {
                text: "diag",
                link: "/cli/diag",
                collapsed: true,
                items: [
                  { text: "check", link: "/cli/diag/check" },
                  { text: "delta", link: "/cli/diag/delta" },
                  { text: "fix", link: "/cli/diag/fix" },
                ],
              },
              {
                text: "edit",
                link: "/cli/edit/",
                collapsed: true,
                items: [
                  { text: "replace", link: "/cli/edit/replace" },
                  { text: "hash", link: "/cli/edit/hash" },
                ],
              },
              {
                text: "graph",
                link: "/cli/graph/",
                collapsed: true,
                items: [
                  { text: "explore", link: "/cli/graph/explore" },
                  { text: "symbols", link: "/cli/graph/symbols" },
                  { text: "callers", link: "/cli/graph/callers" },
                  { text: "callees", link: "/cli/graph/callees" },
                  { text: "impact", link: "/cli/graph/impact" },
                  { text: "affected", link: "/cli/graph/affected" },
                  { text: "index", link: "/cli/graph/index/" },
                  { text: "status", link: "/cli/graph/status" },
                  { text: "prune", link: "/cli/graph/prune" },
                ],
              },
              { text: "handoff", link: "/cli/handoff" },
              { text: "pickup", link: "/cli/pickup" },
              {
                text: "pr",
                link: "/cli/pr/",
                collapsed: true,
                items: [
                  { text: "status", link: "/cli/pr/status" },
                  {
                    text: "threads",
                    link: "/cli/pr/threads",
                    collapsed: true,
                    items: [
                      { text: "reply", link: "/cli/pr/threads/reply" },
                      { text: "resolve", link: "/cli/pr/threads/resolve" },
                      { text: "unresolve", link: "/cli/pr/threads/unresolve" },
                      { text: "apply", link: "/cli/pr/threads/apply" },
                    ],
                  },
                ],
              },
              { text: "read", link: "/cli/read" },
              { text: "review-package", link: "/cli/review-package" },
              { text: "search", link: "/cli/search" },
              { text: "test", link: "/cli/test" },
              {
                text: "wait",
                link: "/cli/wait/",
                collapsed: true,
                items: [
                  { text: "ci", link: "/cli/wait/ci" },
                  { text: "run", link: "/cli/wait/run" },
                  { text: "pr", link: "/cli/wait/pr" },
                  { text: "file", link: "/cli/wait/file" },
                  { text: "log", link: "/cli/wait/log" },
                  { text: "port", link: "/cli/wait/port" },
                  { text: "http", link: "/cli/wait/http" },
                  { text: "pid", link: "/cli/wait/pid" },
                  { text: "cmd", link: "/cli/wait/cmd" },
                ],
              },
              {
                text: "ci",
                link: "/cli/ci/",
                collapsed: true,
                items: [{ text: "run", link: "/cli/ci/run" }],
              },
              {
                text: "rule",
                link: "/cli/rule/",
                collapsed: true,
                items: [
                  { text: "list", link: "/cli/rule/list" },
                  { text: "test", link: "/cli/rule/test" },
                  { text: "schema", link: "/cli/rule/schema" },
                  { text: "types", link: "/cli/rule/types" },
                  { text: "trust", link: "/cli/rule/trust" },
                  { text: "untrust", link: "/cli/rule/untrust" },
                  { text: "new", link: "/cli/rule/new" },
                  { text: "promote", link: "/cli/rule/promote" },
                  { text: "log", link: "/cli/rule/log" },
                  { text: "gap", link: "/cli/rule/gap" },
                ],
              },
              {
                text: "run",
                link: "/cli/run",
                collapsed: true,
                items: [
                  { text: "test", link: "/cli/run/test" },
                  { text: "types", link: "/cli/run/types" },
                ],
              },
              {
                text: "wind",
                link: "/cli/wind/",
                collapsed: true,
                items: [
                  { text: "show", link: "/cli/wind/show" },
                  { text: "dismiss", link: "/cli/wind/dismiss" },
                  { text: "adopt", link: "/cli/wind/adopt" },
                  { text: "digest", link: "/cli/wind/digest" },
                  { text: "review", link: "/cli/wind/review" },
                  { text: "propose", link: "/cli/wind/propose" },
                ],
              },
              { text: "unwind", link: "/cli/unwind" },
              { text: "share", link: "/cli/share" },
              { text: "agent-help", link: "/cli/agent-help" },
              { text: "skill", link: "/cli/skill" },
              { text: "setup", link: "/cli/setup" },
              { text: "completion", link: "/cli/completion" },
            ],
          },
        ],
      },
    ],
    outline: { level: [2, 3] },
    socialLinks: [
      { icon: "github", link: "https://github.com/jdx/jouet" },
    ],
    editLink: {
      // The CLI reference is generated from the usage-rs declaration in
      // src/main.rs, so that is where its pages are edited.
      pattern: ({ filePath }) => {
        const source = filePath.startsWith("cli/")
          ? "src/main.rs"
          : `docs/${filePath}`;
        return `https://github.com/jdx/jouet/edit/main/${source}`;
      },
      text: "Edit this page on GitHub",
    },
    search: { provider: "local" },
    footer: false,
  },
  head: [
    ["link", { rel: "icon", href: "/favicon.ico", sizes: "48x48" }],
    ["link", { rel: "icon", href: "/favicon.svg", type: "image/svg+xml" }],
    [
      "link",
      {
        rel: "icon",
        href: "/favicon-32x32.png",
        type: "image/png",
        sizes: "32x32",
      },
    ],
    [
      "link",
      {
        rel: "icon",
        href: "/favicon-16x16.png",
        type: "image/png",
        sizes: "16x16",
      },
    ],
    [
      "link",
      {
        rel: "apple-touch-icon",
        href: "/apple-touch-icon.png",
        sizes: "180x180",
      },
    ],
    ["link", { rel: "manifest", href: "/site.webmanifest" }],
    [
      "meta",
      {
        name: "theme-color",
        media: "(prefers-color-scheme: light)",
        content: "#fffaf2",
      },
    ],
    [
      "meta",
      {
        name: "theme-color",
        media: "(prefers-color-scheme: dark)",
        content: "#16171c",
      },
    ],
    ["meta", { property: "og:site_name", content: "jouet" }],
    ["meta", { property: "og:locale", content: "en_US" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    ["meta", { name: "twitter:site", content: "@jdxcode" }],
  ],
  transformHead({ pageData, title, description, siteConfig }) {
    const heading = pageData.title || "jouet";
    const card = socialCard(heading, {
      home: pageData.frontmatter.layout === "home",
    });
    writeSocialCard(siteConfig.outDir, card);
    const image = new URL(card.path, `${siteUrl}/`).toString();
    const imageAlt = `${heading} — jouet docs`;
    const url = new URL(
      pageData.relativePath.replace(/index\.md$/, "").replace(/\.md$/, ""),
      `${siteUrl}/`,
    ).toString();

    return [
      ["link", { rel: "canonical", href: url }],
      ["meta", { property: "og:url", content: url }],
      ["meta", { property: "og:image", content: image }],
      // Structured properties follow the image they describe (ogp.me).
      ["meta", { property: "og:image:width", content: "1200" }],
      ["meta", { property: "og:image:height", content: "630" }],
      ["meta", { property: "og:image:alt", content: imageAlt }],
      ["meta", { name: "twitter:image", content: image }],
      ["meta", { name: "twitter:image:alt", content: imageAlt }],
      ["meta", { property: "og:title", content: title }],
      ["meta", { property: "og:description", content: description }],
      ["meta", { name: "twitter:title", content: title }],
      ["meta", { name: "twitter:description", content: description }],
      [
        "script",
        { type: "application/ld+json" },
        JSON.stringify({
          "@context": "https://schema.org",
          "@type": "WebPage",
          name: title,
          description,
          url,
          isPartOf: { "@type": "WebSite", name: "jouet", url: siteUrl },
        }),
      ],
    ];
  },
});
