// Zero-dep icon generator (DESIGN.md §3.1). Hand-rolled PNG (node:zlib) + ICO.
// Glyph = miniature chart-recorder lane: horizontal baseline, small trace blip,
// two ticks. Flat ink, no AA (instrument look). Run: node scripts/gen-icons.mjs
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(fileURLToPath(import.meta.url), "..", "..", "src-tauri");
const RES = path.join(ROOT, "resources");
const ICONS = path.join(ROOT, "icons");

// --- colors (DESIGN §6) ---
const INK = [28, 33, 26, 255];        // #1C211A  dark glyph (for light taskbar)
const LIGHT = [230, 225, 231, 255];   // #E6E1E7  light glyph (for dark taskbar)
const PAPER = [227, 232, 217, 255];   // #E3E8D9  LEDGER paper (app icon field)

// --- pixel buffer helpers ---
const mkbuf = (W, H, bg = [0, 0, 0, 0]) => {
  const b = Buffer.alloc(W * H * 4);
  for (let i = 0; i < W * H; i++) b.set(bg, i * 4);
  return b;
};
const px = (b, W, H, x, y, c) => {
  x = Math.round(x); y = Math.round(y);
  if (x < 0 || y < 0 || x >= W || y >= H) return;
  b.set(c, (y * W + x) * 4);
};
const rect = (b, W, H, x, y, w, h, c) => {
  for (let j = 0; j < h; j++) for (let i = 0; i < w; i++) px(b, W, H, x + i, y + j, c);
};
// thick line via t*t stamp along Bresenham path
function line(b, W, H, x0, y0, x1, y1, t, c) {
  x0 = Math.round(x0); y0 = Math.round(y0); x1 = Math.round(x1); y1 = Math.round(y1);
  const dx = Math.abs(x1 - x0), dy = -Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1, sy = y0 < y1 ? 1 : -1;
  let err = dx + dy, o = Math.floor(t / 2);
  for (;;) {
    rect(b, W, H, x0 - o, y0 - o, t, t, c);
    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x0 += sx; }
    if (e2 <= dx) { err += dx; y0 += sy; }
  }
}

// The chart-recorder glyph. solidBlip => filled (heavier ink) recording state;
// strike => 1px diagonal through the blip (mic error).
function drawGlyph(b, W, H, col, { solidBlip = false, strike = false } = {}) {
  const t = Math.max(1, Math.round(W / 16));
  const base = Math.round(H * 0.58);
  const x0 = Math.round(W * 0.14), x1 = Math.round(W * 0.86);
  // baseline
  rect(b, W, H, x0, base, x1 - x0, t, col);
  // two ticks hanging below the baseline
  const tickH = Math.max(2, Math.round(H * 0.16));
  for (const fx of [0.30, 0.68]) {
    const tx = Math.round(W * fx);
    rect(b, W, H, tx, base + t, t, tickH, col);
  }
  // trace blip: a small peak rising above the baseline near center
  const bx0 = Math.round(W * 0.42), bxp = Math.round(W * 0.50), bx1 = Math.round(W * 0.58);
  const peakY = base - Math.round(H * 0.22);
  if (solidBlip) {
    // filled peak: vertical spans from baseline up to the ^ envelope
    for (let x = bx0; x <= bx1; x++) {
      const frac = x <= bxp ? (x - bx0) / (bxp - bx0 || 1) : (bx1 - x) / (bx1 - bxp || 1);
      const top = Math.round(base - frac * (base - peakY));
      rect(b, W, H, x, top, t, base - top + t, col);
    }
  } else {
    line(b, W, H, bx0, base, bxp, peakY, t, col);
    line(b, W, H, bxp, peakY, bx1, base, t, col);
  }
  if (strike) line(b, W, H, bx0, peakY, bx1, base + tickH, Math.max(1, t), col);
}

// --- PNG encoder ---
let CRC;
function crc32(buf) {
  if (!CRC) {
    CRC = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      CRC[n] = c >>> 0;
    }
  }
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(Buffer.concat([Buffer.from(type, "ascii"), data])), 8 + data.length);
  return out;
}
function encodePng(W, H, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4);
  ihdr[8] = 8; ihdr[9] = 6; // 8-bit RGBA
  const stride = W * 4;
  const raw = Buffer.alloc((stride + 1) * H);
  for (let y = 0; y < H; y++) rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  const idat = zlib.deflateSync(raw, { level: 9 });
  return Buffer.concat([sig, chunk("IHDR", ihdr), chunk("IDAT", idat), chunk("IEND", Buffer.alloc(0))]);
}
function encodeIco(imgs) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2); header.writeUInt16LE(imgs.length, 4);
  const dir = Buffer.alloc(16 * imgs.length);
  let offset = 6 + dir.length;
  imgs.forEach((img, i) => {
    const e = i * 16;
    dir[e] = img.size >= 256 ? 0 : img.size;
    dir[e + 1] = img.size >= 256 ? 0 : img.size;
    dir.writeUInt16LE(1, e + 4);   // planes
    dir.writeUInt16LE(32, e + 6);  // bpp
    dir.writeUInt32LE(img.png.length, e + 8);
    dir.writeUInt32LE(offset, e + 12);
    offset += img.png.length;
  });
  return Buffer.concat([header, dir, ...imgs.map((i) => i.png)]);
}

// rounded-2px paper square for the app icon
function paperSquare(W, H) {
  const b = mkbuf(W, H, PAPER);
  const r = 2;
  for (const [cx, cy, sx, sy] of [[0, 0, 1, 1], [W - 1, 0, -1, 1], [0, H - 1, 1, -1], [W - 1, H - 1, -1, -1]]) {
    for (let i = 0; i < r; i++) for (let j = 0; j < r; j++) {
      if ((i - r + 0.5) ** 2 + (j - r + 0.5) ** 2 > r * r) px(b, W, H, cx + sx * i, cy + sy * j, [0, 0, 0, 0]);
    }
  }
  return b;
}

fs.mkdirSync(RES, { recursive: true });
fs.mkdirSync(ICONS, { recursive: true });

// --- tray icons: 3 states x 2 themes, 16px transparent ---
const states = { idle: {}, rec: { solidBlip: true }, err: { strike: true } };
for (const [name, opt] of Object.entries(states)) {
  for (const [theme, col] of [["light", LIGHT], ["dark", INK]]) {
    const b = mkbuf(16, 16);
    drawGlyph(b, 16, 16, col, opt);
    fs.writeFileSync(path.join(RES, `tray-${name}-${theme}.png`), encodePng(16, 16, b));
    // Raw RGBA sidecar: tray.rs loads these via Image::new (no image-png
    // feature, no runtime decode).
    fs.writeFileSync(path.join(RES, `tray-${name}-${theme}.rgba`), b);
    console.log(`tray-${name}-${theme}.png + .rgba  16x16`);
  }
}

// --- app icon: ink glyph on LEDGER paper, ICO {16,32,48,256} + 128 PNG ---
function appIcon(size) {
  const b = paperSquare(size, size);
  drawGlyph(b, size, size, INK);
  return encodePng(size, size, b);
}
const ico = encodeIco([16, 32, 48, 256].map((s) => ({ size: s, png: appIcon(s) })));
fs.writeFileSync(path.join(ICONS, "icon.ico"), ico);
console.log(`icon.ico  ${ico.length} bytes  (16/32/48/256)`);
fs.writeFileSync(path.join(ICONS, "128x128.png"), appIcon(128));
console.log(`128x128.png`);
