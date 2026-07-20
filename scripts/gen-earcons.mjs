// Zero-dep earcon generator (DESIGN.md §9). 48kHz 16-bit mono WAVs, dry &
// mechanical: an instrument engaging, not a chime. Peak ~0.12 (quiet), 3ms
// raised-cosine fade edges, no reverb. Run: node scripts/gen-earcons.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const RATE = 48000;
const OUT = path.resolve(fileURLToPath(import.meta.url), "..", "..", "src-tauri", "resources");

const buf = (durMs) => new Float32Array(Math.round((RATE * durMs) / 1000));
const noise = () => Math.random() * 2 - 1;

// Damped sinusoid + optional sharp noise transient = a dry mechanical click.
function damped(freq, durMs, decayK, noiseAmt = 0) {
  const a = buf(durMs);
  for (let i = 0; i < a.length; i++) {
    const t = i / RATE;
    let v = Math.sin(2 * Math.PI * freq * t) * Math.exp(-decayK * t);
    if (noiseAmt > 0) v += noise() * noiseAmt * Math.exp(-decayK * 3 * t);
    a[i] = v;
  }
  return a;
}

// 3ms raised-cosine edges — kills buffer-boundary DC pops without erasing attack.
function fades(a, ms = 3) {
  const f = Math.round((RATE * ms) / 1000);
  for (let i = 0; i < f && i < a.length; i++) {
    const g = 0.5 - 0.5 * Math.cos((Math.PI * i) / f);
    a[i] *= g;
    a[a.length - 1 - i] *= g;
  }
  return a;
}

function normalize(a, peak) {
  let m = 0;
  for (const v of a) m = Math.max(m, Math.abs(v));
  if (m > 0) for (let i = 0; i < a.length; i++) a[i] *= peak / m;
  return a;
}

function concat(...arrs) {
  const n = arrs.reduce((s, x) => s + x.length, 0);
  const out = new Float32Array(n);
  let o = 0;
  for (const x of arrs) { out.set(x, o); o += x.length; }
  return out;
}

function writeWav(name, f32) {
  const n = f32.length;
  const b = Buffer.alloc(44 + n * 2);
  b.write("RIFF", 0); b.writeUInt32LE(36 + n * 2, 4); b.write("WAVE", 8);
  b.write("fmt ", 12); b.writeUInt32LE(16, 16); b.writeUInt16LE(1, 20); b.writeUInt16LE(1, 22);
  b.writeUInt32LE(RATE, 24); b.writeUInt32LE(RATE * 2, 28); b.writeUInt16LE(2, 32); b.writeUInt16LE(16, 34);
  b.write("data", 36); b.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) {
    const s = Math.max(-1, Math.min(1, f32[i]));
    b.writeInt16LE(Math.round(s * 32767), 44 + i * 2);
  }
  const p = path.join(OUT, name);
  fs.writeFileSync(p, b);
  console.log(`${name}  ${b.length} bytes  ${(n / RATE * 1000).toFixed(0)}ms`);
}

fs.mkdirSync(OUT, { recursive: true });

// NOTE: underscore names to match the landed consumer audio/cues.rs CUE_FILES
// (["cue_start.wav", ...]) — deviates from the task's hyphenated spec; the
// committed reader is the authority.
// Start: single ~1kHz damped mechanical click (relay seat), <=60ms.
writeWav("cue_start.wav", normalize(fades(damped(1000, 55, 70, 0.35)), 0.12));
// Stop: same click pitched ~15% down (release of the mechanism), <=60ms.
writeWav("cue_stop.wav", normalize(fades(damped(850, 55, 70, 0.35)), 0.12));
// Discard: two rapid dry ticks, <=80ms.
{
  const tick = () => damped(1600, 14, 220, 0.5);
  writeWav("cue_discard.wav", normalize(fades(concat(tick(), buf(18), tick())), 0.12));
}
// Error: dull ~350Hz thunk, lower & quieter than start, <=100ms.
writeWav("cue_error.wav", normalize(fades(damped(350, 90, 26, 0.2)), 0.10));
