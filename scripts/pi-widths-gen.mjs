#!/usr/bin/env node
// Generate `tests/golden/pi-widths.json` from pi's own text functions (R18).
//
// pi's `packages/tui/src/utils.ts` exports `visibleWidth`,
// `truncateToWidth`, and `wrapTextWithAnsi` - the ANSI-aware visual-column
// layer LCA's `lca-tui::text` ports. This script feeds the `utils.md` case
// taxonomy through pi's functions and writes the answers;
// `crates/lca-tui/tests/pi_widths.rs` diffs LCA's answers against the file
// and fails on drift.
//
// It imports pi's TypeScript through Node's type stripping, so the run
// needs `get-east-asian-width` resolvable from the imported file. The
// checked-in JSON is the artifact that matters; regenerate with:
//
//   cp ~/gits/pi/packages/tui/src/utils.ts /tmp/pi-width/utils.ts
//   (cd /tmp/pi-width && npm install get-east-asian-width)
//   PI_TUI_UTILS=/tmp/pi-width/utils.ts node scripts/pi-widths-gen.mjs
//
// Never run this against the owner's pi tree in a way that writes to it.

import { writeFileSync } from "node:fs";

const utilsPath =
  process.env.PI_TUI_UTILS ??
  `${process.env.HOME}/gits/pi/packages/tui/src/utils.ts`;
const { visibleWidth, truncateToWidth, wrapTextWithAnsi } = await import(
  utilsPath
);

const widthCases = [
  "",
  "hello",
  "hello world",
  // Wide characters.
  "你好世界",
  "mixed ascii 你好 done",
  // Emoji, including a run and an RGI sequence.
  "🎉🚀",
  "👨‍👩‍👧‍👦",
  "🇺🇸🇯🇵",
  // Regional indicators mid-pair (streaming split).
  "\u{1F1FA}",
  "\u{1F1FA}\u{1F1F8}",
  // Combining marks.
  "e\u0301",
  "a\u0301b\u0302c\u0303",
  // Thai / Lao AM vowels.
  "กำ",
  "ກຳ",
  // Tabs (pi expands to three spaces).
  "a\tb",
  "\t",
  // ANSI styling runs (stripped for width).
  "\x1b[31mred\x1b[0m",
  "\x1b[1;32mbold green\x1b[0m plain",
  // OSC 8 hyperlink (the text is the visible part).
  "\x1b]8;;https://pi.dev\x07pi\x1b]8;;\x07",
  // A CSI cursor move (stripped; width counts only the text).
  "\x1b[2Kcleared",
];

const truncationCases = [
  { text: "hello world", max: 5 },
  { text: "hello world", max: 11 },
  { text: "hello world", max: 20 },
  { text: "你好世界", max: 4 },
  { text: "你好世界", max: 5 },
  { text: "🎉🚀", max: 3 },
  { text: "\x1b[31mhello\x1b[0m world", max: 7 },
  { text: "héllo wörld", max: 8 },
];

const wrapCases = [
  { text: "the quick brown fox jumps over the lazy dog", width: 12 },
  { text: "hello world", width: 5 },
  { text: "你好世界你好世界", width: 4 },
  { text: "a-very-long-token-without-spaces", width: 8 },
  { text: "one\ntwo\nthree", width: 10 },
];

const out = {
  widths: widthCases.map((text) => ({ text, width: visibleWidth(text) })),
  truncations: truncationCases.map(({ text, max }) => ({
    text,
    max,
    result: truncateToWidth(text, max, "...", false),
  })),
  wraps: wrapCases.map(({ text, width }) => ({
    text,
    width,
    lines: wrapTextWithAnsi(text, width),
  })),
};
const target = new URL("../tests/golden/pi-widths.json", import.meta.url);
writeFileSync(target, JSON.stringify(out, null, 2) + "\n");
console.log(
  `pi-widths: wrote ${out.widths.length} widths, ${out.truncations.length} truncations, ${out.wraps.length} wraps`,
);
