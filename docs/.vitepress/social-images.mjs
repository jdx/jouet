import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const fontFile = fileURLToPath(
  new URL("./fonts/SpaceGrotesk.ttf", import.meta.url),
);
const logo = readFileSync(
  new URL("../public/logo.svg", import.meta.url),
  "utf8",
);
const logoHref = `data:image/svg+xml;base64,${Buffer.from(logo).toString("base64")}`;
// The docs' dark palette and the robot's three accents.
const colors = {
  bg: "#16171c",
  plate: "#1f2027",
  rule: "#33343c",
  text: "#efeae4",
  muted: "#bdb6ae",
  coral: "#ef6a55",
  brass: "#f2b84b",
  teal: "#2a9d8f",
};
const tagline = "wind-up tools for coding agents";
// resvg draws only the variable font's default (light) instance, so a stroke
// in the fill colour stands in for the semibold the site uses.
const bold = (color, width) =>
  `fill="${color}" stroke="${color}" stroke-width="${width}" stroke-linejoin="round"`;
const escapeXml = (value) =>
  String(value).replace(
    /[<>&"']/g,
    (c) =>
      ({
        "<": "&lt;",
        ">": "&gt;",
        "&": "&amp;",
        '"': "&quot;",
        "'": "&apos;",
      })[c],
  );

const fontOptions = { fontFiles: [fontFile], loadSystemFonts: false };
const widths = new Map();
export function textWidth(text, size) {
  const key = `${size}:${text}`;
  if (!widths.has(key)) {
    const measure = new Resvg(
      `<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="200"><text y="100" font-family="Space Grotesk" font-size="${size}">${escapeXml(text)}</text></svg>`,
      { font: fontOptions },
    );
    widths.set(key, measure.innerBBox()?.width ?? 0);
  }
  return widths.get(key);
}

// Measure with the same bundled font used to render, including long CLI names.
export function wrapTitle(title, size = 62, width = 720) {
  const words = String(title)
    .trim()
    .split(/\s+/)
    .flatMap((word) => {
      const chunks = [];
      let chunk = "";
      for (const letter of word) {
        if (chunk && textWidth(chunk + letter, size) > width) {
          chunks.push(chunk);
          chunk = "";
        }
        chunk += letter;
      }
      if (chunk) chunks.push(chunk);
      return chunks;
    });
  const lines = [];
  let line = "";
  for (const word of words) {
    if (line && textWidth(`${line} ${word}`, size) > width) {
      lines.push(line);
      line = word;
    } else line = line ? `${line} ${word}` : word;
  }
  if (line) lines.push(line);
  return lines;
}

// The tin stripe along the top edge, as on every docs page.
const stripe = `<rect width="400" height="10" fill="${colors.coral}"/>
    <rect x="400" width="400" height="10" fill="${colors.brass}"/>
    <rect x="800" width="400" height="10" fill="${colors.teal}"/>`;

// The robot on its round plate, centred on (cx, cy).
const robot = (cx, cy, size) => {
  const r = size * 0.62;
  return `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${colors.plate}"/>
    <circle cx="${cx}" cy="${cy}" r="${r}" fill="none" stroke="${colors.coral}" stroke-opacity="0.35" stroke-width="2" stroke-dasharray="6 8"/>
    <image x="${cx - size / 2}" y="${cy - size / 2 - size * 0.04}" width="${size}" height="${size}" xlink:href="${logoHref}"/>`;
};

function pageSvg(title) {
  let size = 62;
  let lines = wrapTitle(title, size);
  if (lines.length > 4) {
    size = 54;
    lines = wrapTitle(title, size);
  }
  if (lines.length > 4) {
    lines = lines.slice(0, 4);
    while (textWidth(`${lines[3]}…`, size) > 720)
      lines[3] = Array.from(lines[3]).slice(0, -1).join("");
    lines[3] += "…";
  }
  const start = 298 - ((lines.length - 1) * (size + 14)) / 2;
  const heading = lines
    .map(
      (line, i) =>
        `<text x="64" y="${start + i * (size + 14)}" font-size="${size}" ${bold(colors.text, 1.4)}>${escapeXml(line)}</text>`,
    )
    .join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1200" height="630" viewBox="0 0 1200 630">
    <rect width="1200" height="630" fill="${colors.bg}"/>
    ${stripe}
    ${robot(1000, 280, 250)}
    <g font-family="Space Grotesk">
      <text x="64" y="92" font-size="30" fill="${colors.coral}">jouet / docs</text>
      ${heading}
      <path d="M64 508 H1136" stroke="${colors.rule}"/>
      <text x="64" y="564" font-size="26" fill="${colors.muted}">jouet.jdx.dev</text>
      <text x="1136" y="564" text-anchor="end" font-size="26" fill="${colors.coral}">${tagline}</text>
    </g>
  </svg>`;
}

// The home page gets the brand card: the robot large, the name and the tagline.
function homeSvg() {
  return `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1200" height="630" viewBox="0 0 1200 630">
    <rect width="1200" height="630" fill="${colors.bg}"/>
    ${stripe}
    ${robot(318, 318, 330)}
    <g font-family="Space Grotesk">
      <text x="610" y="292" font-size="132" letter-spacing="-5" ${bold(colors.text, 3)}>jouet</text>
      <text x="616" y="180" font-size="26" fill="${colors.muted}">“zhweh” · French for “toy”</text>
      <text x="616" y="366" font-size="40" ${bold(colors.coral, 0.8)}>Spend fewer tokens.</text>
      <text x="616" y="418" font-size="40" ${bold(colors.coral, 0.8)}>Get there faster.</text>
      <text x="616" y="520" font-size="26" fill="${colors.muted}">jouet.jdx.dev</text>
    </g>
  </svg>`;
}

export function socialCard(title, { home = false } = {}) {
  const svg = home ? homeSvg() : pageSvg(title);
  const hash = createHash("sha256")
    .update(svg)
    .update(readFileSync(fontFile))
    .digest("hex")
    .slice(0, 16);
  return { svg, path: `social/${hash}.png` };
}

export function writeSocialCard(outDir, card) {
  const image = new Resvg(card.svg, { font: fontOptions }).render().asPng();
  mkdirSync(resolve(outDir, "social"), { recursive: true });
  writeFileSync(resolve(outDir, card.path), image);
}
