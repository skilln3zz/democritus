// The ferrofluid orb (design-neumorphic/NEU-SPEC.md): a glossy black drop of
// magnetic fluid whose spikes rise and sink. Built once and used in three
// places: small beside the reply being generated in chat, small beside the
// coding agent's reply, and large on the Training tab.
//
// It is drawn in 3D with raw WebGL2 (tuned in the Ferrofluid lab, preset base
// "calm"): a unit icosphere whose vertices the vertex shader pushes out along
// their own direction, by a wobbling body plus 51 cone-shaped spikes. Spikes
// are slots, not fixed places: one that sinks back in comes out somewhere new.
// Without WebGL2 (or if its shaders fail), the 2D drawing of the mockups is
// used instead (NeuCode.dc.html for light backgrounds, NeuTrain.dc.html for
// dark ones). localStorage "democritus.orbRenderer" = "2d" forces it, to compare.
//
// What drives it is up to the page: each event() (a thinking token, a tool
// step, a training step) raises a target intensity, which decays every frame
// and is eased toward (over about a second). Two modes:
//   active  spikes up, follows the events, bursts every few seconds
//   calm    a quiet drop breathing slowly between 0.08 and 0.12 with soft bumps
//           (chat while getting ready and answering, the agent while it waits for
//           you or is done, the Training tab while idle or paused); no bursts
// The glow follows the work, not the shape: only events light it, and it fades
// within about a second when they stop.
//
// An orb is never completely still while it can be seen. Its loop pauses only
// while it can't be: the window is hidden or minimised, or the orb is off screen
// (another page or view, or scrolled out of sight). The quiet drop is drawn at
// 30 fps; everything is drawn at the screen's devicePixelRatio (at most 2).

export type OrbMode = 'active' | 'calm';

export interface OrbTuning {
  /** Per-frame (60 fps) decay of the target intensity, and its floor while active. */
  decay: number;
  floor: number;
  /** Target in calm mode, whether it breathes slowly around it, and how deep (share of calm). */
  calm: number;
  breathe: boolean;
  breatheDepth: number;
  /** Per-frame easing of the drawn intensity toward the target (0.16 ≈ instant, 0.06 ≈ a second). */
  ease: number;
  /** Frame-rate cap while not active (0 = none). */
  idleFps: number;
  /** First burst after this many ms of activity, then every min–max ms. */
  burstFirst: number;
  burstMin: number;
  burstMax: number;
  /** Per-frame decay of a burst while active, and otherwise. */
  burstDecayActive: number;
  burstDecayIdle: number;
}

/** Chat: thinking tokens drive it (NeuChat.dc.html). */
export const CHAT_TUNING: OrbTuning = {
  decay: 0.962,
  floor: 0.22,
  calm: 0.1,
  breathe: true,
  breatheDepth: 0.2,
  ease: 0.06,
  idleFps: 30,
  burstFirst: 1800,
  burstMin: 2200,
  burstMax: 5200,
  burstDecayActive: 0.95,
  burstDecayIdle: 0.88,
};

/** The coding agent: tokens and tool steps while it works (NeuCode.dc.html). */
export const AGENT_TUNING: OrbTuning = {
  decay: 0.955,
  floor: 0.26,
  calm: 0.1,
  breathe: true,
  breatheDepth: 0.2,
  ease: 0.06,
  idleFps: 30,
  burstFirst: 2000,
  burstMin: 3000,
  burstMax: 7000,
  burstDecayActive: 0.95,
  burstDecayIdle: 0.88,
};

/** The Training tab: training steps (NeuTrain.dc.html). */
export const TRAINING_TUNING: OrbTuning = {
  decay: 0.952,
  floor: 0.34,
  calm: 0.1,
  breathe: true,
  breatheDepth: 0.2,
  ease: 0.06,
  idleFps: 30,
  burstFirst: 2500,
  burstMin: 3500,
  burstMax: 8500,
  burstDecayActive: 0.955,
  burstDecayIdle: 0.9,
};

/**
 * The 3D look, from the Ferrofluid lab (preset base "calm"). Shape and shading
 * only: the lab's decay and idle stay per page, in the tunings above.
 */
const LOOK = {
  spikes: 51,
  length: 0.23,
  body: 0.175,
  width: 0.81,
  sharp: 4,
  base: 0.28,
  sag: 1.03,
  speed: 1,
  snap: 2,
  floor: 0,
  retract: 0,
  bounce: 0.4,
  wander: 0.08,
  irregular: 0.3,
  rot: 0.3,
  wobble: 1,
  tremble: 0,
  heart: 0,
  burstLen: 0.3,
  burstAmp: 0.8,
  /** Scales the time between bursts. */
  burstEvery: 1,
  kick: 1,
  magnet: 0.6,
  gloss: 0.75,
  rim: 0.62,
  halo: 1,
  ember: 0,
};

/** Where a new orb starts before it eases into its first state. */
const START = 0.04;

// Pause orbs that can't be seen; resume them when they can.
const orbOf = new WeakMap<Element, FerroOrb>();
const visibility = new IntersectionObserver((entries) => {
  for (const e of entries) orbOf.get(e.target)?.setOnScreen(e.isIntersecting);
});
const live = new Set<FerroOrb>();
document.addEventListener('visibilitychange', () => {
  for (const orb of live) orb.resume();
});
const TWO = Math.PI * 2;
const CONES = 11;

// One loop for every running orb. It sleeps on a timer until the next orb is
// due and only then asks for an animation frame: on a 300 Hz screen, a calm
// orb then wakes the page 30 times a second, not 300. Active orbs (and a
// flinch or the pointer magnet) are drawn at up to 60 fps.
const ACTIVE_FPS = 60;
const running = new Set<FerroOrb>();
let frameReq = 0;
let sleepTimer = 0;

function schedule() {
  if (frameReq || sleepTimer || running.size === 0) return;
  let due = Infinity;
  for (const o of running) due = Math.min(due, o.dueAt());
  const wait = due - performance.now() - 4;
  if (wait > 1) {
    sleepTimer = window.setTimeout(() => {
      sleepTimer = 0;
      frameReq = requestAnimationFrame(onFrame);
    }, wait);
  } else {
    frameReq = requestAnimationFrame(onFrame);
  }
}

function onFrame(now: number) {
  frameReq = 0;
  for (const o of [...running]) if (now >= o.dueAt() - 4) o.frame(now);
  schedule();
}

function unschedule() {
  if (running.size) return;
  cancelAnimationFrame(frameReq);
  clearTimeout(sleepTimer);
  frameReq = 0;
  sleepTimer = 0;
}

// ---------- 3D: camera and small vector helpers ----------

const EYE: Vec3 = [0, 0.55, 4.2];
/** Canvas height in world units at the orb (30° field of view, 4.2 away). */
const WORLD = 2 * Math.tan((15 * Math.PI) / 180) * 4.2;
const MAX_SPIKES = 96;

type Vec3 = [number, number, number];
const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const norm = (a: Vec3): Vec3 => {
  const l = Math.hypot(a[0], a[1], a[2]) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
};
const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x));

function perspective(fovDeg: number, aspect: number, near: number, far: number): Float32Array {
  const f = 1 / Math.tan((fovDeg * Math.PI) / 360);
  const nf = 1 / (near - far);
  // column-major
  return new Float32Array([f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0]);
}

function lookAt(eye: Vec3, target: Vec3, up: Vec3): Float32Array {
  const z = norm([eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]]);
  const x = norm(cross(up, z));
  const y = cross(z, x);
  return new Float32Array([x[0], y[0], z[0], 0, x[1], y[1], z[1], 0, x[2], y[2], z[2], 0, -dot(x, eye), -dot(y, eye), -dot(z, eye), 1]);
}

const PROJ = perspective(30, 1, 0.1, 20);
const VIEW = lookAt(EYE, [0, 0, 0], [0, 1, 0]);

/** A unit icosphere: positions (also the normals) and triangle indices. */
function icosphere(level: number): { pos: Float32Array; idx: Uint32Array } {
  const t = (1 + Math.sqrt(5)) / 2;
  const v: number[] = [];
  const add = (x: number, y: number, z: number) => {
    const l = Math.hypot(x, y, z);
    v.push(x / l, y / l, z / l);
    return v.length / 3 - 1;
  };
  for (const [x, y, z] of [
    [-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0], [0, -1, t], [0, 1, t],
    [0, -1, -t], [0, 1, -t], [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1],
  ]) add(x, y, z);
  let faces: number[] = [
    0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
    3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
  ];
  for (let l = 0; l < level; l++) {
    const mid = new Map<number, number>();
    const m = (a: number, b: number) => {
      const key = a < b ? a * 1_000_003 + b : b * 1_000_003 + a;
      let i = mid.get(key);
      if (i === undefined) {
        i = add(v[a * 3] + v[b * 3], v[a * 3 + 1] + v[b * 3 + 1], v[a * 3 + 2] + v[b * 3 + 2]);
        mid.set(key, i);
      }
      return i;
    };
    const next: number[] = [];
    for (let f = 0; f < faces.length; f += 3) {
      const [a, b, c] = [faces[f], faces[f + 1], faces[f + 2]];
      const ab = m(a, b);
      const bc = m(b, c);
      const ca = m(c, a);
      next.push(a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca);
    }
    faces = next;
  }
  return { pos: new Float32Array(v), idx: new Uint32Array(faces) };
}

const VERT = `#version 300 es
precision highp float;
in vec3 aN;
uniform mat4 uProj, uView;
uniform mat3 uRot;
uniform vec3 uShift;
uniform vec4 uSpk[96];      // xyz = direction (object space), w = amplitude
uniform vec4 uHW[24];       // half-width (radians) of each cone, 4 per vec4
uniform int uN;
uniform float uR, uS, uBase, uSharp, uTime, uWob, uSag, uMag;
uniform vec3 uMagDir;       // object space
out vec3 vNw; out vec3 vPw; out float vH;
float radiusAt(vec3 n) {
  float t = uTime;
  float r = uR * (1.0 + uWob * (0.055 * sin(3.0 * n.x + 2.0 * n.y + t * 1.9) + 0.038 * sin(4.0 * n.z - 3.0 * n.y - t * 3.0) + 0.022 * sin(6.0 * n.x + 5.0 * n.z + t * 4.6)));
  r += uR * 0.12 * uMag * pow(max(dot(n, uMagDir), 0.0), 3.0);
  for (int i = 0; i < 96; i++) {
    if (i >= uN) break;
    vec4 s = uSpk[i];
    float w = uHW[i >> 2][i & 3];
    float c = dot(n, s.xyz);
    if (c > cos(w)) {
      float s1 = max(1.0 - acos(min(c, 1.0)) / w, 0.0);
      r += uS * s.w * ((1.0 - uBase) * pow(s1, uSharp) + uBase * s1 * s1);
    }
  }
  return r;
}
vec3 shape(vec3 n) { vec3 p = n * radiusAt(n); p.y *= uSag; return p; }
void main() {
  vec3 n = normalize(aN);
  vec3 t1 = normalize(cross(n, abs(n.y) < 0.99 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0)));
  vec3 t2 = cross(n, t1);
  float e = 0.0035;
  vec3 p = shape(n);
  vec3 p1 = shape(normalize(n + t1 * e));
  vec3 p2 = shape(normalize(n + t2 * e));
  vec3 nm = normalize(cross(p1 - p, p2 - p));
  if (dot(nm, n) < 0.0) nm = -nm;
  vH = clamp((length(p) - uR) / (uR * 1.2), 0.0, 1.5);
  vec3 pw = uRot * p + uShift;
  vPw = pw; vNw = uRot * nm;
  gl_Position = uProj * uView * vec4(pw, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
in vec3 vNw; in vec3 vPw; in float vH;
uniform vec3 uCam, uAccent;
uniform float uGloss, uRim, uEmber, uI, uB, uDark, uTime, uHeart, uWork;
out vec4 outColor;
vec3 env(vec3 r) {
  float y = r.y;
  vec3 darkRoom = mix(vec3(0.004), vec3(0.03), smoothstep(-0.3, 1.0, y));
  vec3 lightRoom = mix(vec3(0.05, 0.047, 0.043), vec3(0.42, 0.405, 0.385), smoothstep(-0.4, 0.9, y));
  vec3 c = mix(lightRoom, darkRoom, uDark);
  float s1 = smoothstep(0.93, 0.988, dot(r, normalize(vec3(-0.5, 0.8, 0.6))));
  float s2 = smoothstep(0.965, 0.992, dot(r, normalize(vec3(0.85, 0.35, 0.4))));
  c += vec3(1.0) * s1 * 1.8 + vec3(0.55) * s2;
  c += vec3(0.07) * exp(-abs(y + 0.05) * 28.0);
  c += uAccent * smoothstep(0.55, 0.95, dot(r, normalize(vec3(0.7, -0.6, -0.1)))) * uRim * (0.15 + 1.2 * uWork + 0.6 * uB);
  return c;
}
void main() {
  vec3 N = normalize(vNw);
  vec3 V = normalize(uCam - vPw);
  if (dot(N, V) < 0.0) N = -N;
  float ndv = max(dot(N, V), 0.0);
  float F = 0.04 + 0.96 * pow(1.0 - ndv, 5.0);
  vec3 R = reflect(-V, N);
  vec3 col = vec3(0.01) * (0.3 + 0.7 * max(dot(N, normalize(vec3(-0.4, 0.8, 0.5))), 0.0));
  col += env(R) * mix(0.04, 1.0, F) * (0.25 + uGloss * 1.2);
  float valley = 1.0 - smoothstep(0.0, 0.32, vH);
  float flick = 0.72 + 0.28 * sin(uTime * 13.0 + vPw.x * 9.0) * sin(uTime * 7.3 + vPw.y * 11.0);
  vec3 ember = vec3(1.0, 0.13, 0.04) * uEmber * valley * uWork * (1.5 + 0.9 * uB + 0.4 * uHeart) * flick * (1.0 - 0.7 * F);
  col += ember * mix(0.3, 0.6, uDark);
  col = col / (1.0 + col);
  outColor = vec4(pow(col, vec3(1.0 / 2.2)), 1.0);
}`;

/** What one orb hands the renderer for a frame. */
interface Frame {
  size: number;
  rot: Float32Array;
  /** The shiver (tremble). */
  shift: Vec3;
  spk: Float32Array;
  hw: Float32Array;
  n: number;
  r: number;
  s: number;
  time: number;
  mag: number;
  magDir: Vec3;
  accent: Vec3;
  dark: boolean;
  intensity: number;
  burst: number;
  heart: number;
  work: number;
}

/**
 * One WebGL2 context per mesh size, shared by every orb of that size, so a
 * long chat or agent session never runs out of WebGL contexts. Each frame is
 * handed to the orb's own canvas as a GPU bitmap (OffscreenCanvas →
 * ImageBitmap → "bitmaprenderer"): no copy, and no read back to the CPU.
 */
class Renderer {
  readonly canvas: OffscreenCanvas | HTMLCanvasElement = typeof OffscreenCanvas === 'undefined' ? document.createElement('canvas') : new OffscreenCanvas(88, 88);
  private gl: WebGL2RenderingContext;
  private count: number;
  private u: Record<string, WebGLUniformLocation | null> = {};
  lost = false;

  private static cache = new Map<number, Renderer | null>();

  static get(level: number): Renderer | null {
    if (!Renderer.cache.has(level)) {
      let r: Renderer | null = null;
      try {
        r = new Renderer(level);
      } catch (e) {
        console.warn('Ferrofluid orb: WebGL2 is not available, using the 2D orb.', e);
      }
      Renderer.cache.set(level, r);
    }
    const r = Renderer.cache.get(level) ?? null;
    return r && !r.lost ? r : null;
  }

  private constructor(level: number) {
    const gl = this.canvas.getContext('webgl2', { alpha: true, antialias: true, premultipliedAlpha: true, depth: true, powerPreference: 'low-power' }) as WebGL2RenderingContext | null;
    if (!gl) throw new Error('no WebGL2');
    this.gl = gl;
    (this.canvas as EventTarget).addEventListener('webglcontextlost', (e) => {
      e.preventDefault();
      this.lost = true; // the orbs fall back to 2D
    });
    const shader = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
      return s;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, shader(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, shader(gl.FRAGMENT_SHADER, FRAG));
    gl.bindAttribLocation(prog, 0, 'aN');
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(prog) ?? 'link');
    for (const name of ['uProj', 'uView', 'uRot', 'uShift', 'uSpk', 'uHW', 'uN', 'uR', 'uS', 'uBase', 'uSharp', 'uTime', 'uWob', 'uSag', 'uMag', 'uMagDir', 'uCam', 'uAccent', 'uGloss', 'uRim', 'uEmber', 'uI', 'uB', 'uDark', 'uHeart', 'uWork']) {
      this.u[name] = gl.getUniformLocation(prog, name);
    }
    const mesh = icosphere(level);
    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, mesh.pos, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, mesh.idx, gl.STATIC_DRAW);
    this.count = mesh.idx.length;
    gl.useProgram(prog);
    // Fixed for every frame.
    gl.uniformMatrix4fv(this.u.uProj, false, PROJ);
    gl.uniformMatrix4fv(this.u.uView, false, VIEW);
    gl.uniform1f(this.u.uBase, LOOK.base);
    gl.uniform1f(this.u.uSharp, 1.4 + LOOK.sharp * 0.3);
    gl.uniform1f(this.u.uWob, LOOK.wobble);
    gl.uniform1f(this.u.uSag, LOOK.sag);
    gl.uniform3f(this.u.uCam, EYE[0], EYE[1], EYE[2]);
    gl.uniform1f(this.u.uGloss, LOOK.gloss);
    gl.uniform1f(this.u.uRim, LOOK.rim);
    gl.uniform1f(this.u.uEmber, LOOK.ember);
    gl.enable(gl.DEPTH_TEST);
    gl.disable(gl.CULL_FACE);
    gl.clearColor(0, 0, 0, 0);
  }

  /**
   * Draws one frame. Returns it as a bitmap (OffscreenCanvas), true when it is
   * on this.canvas until the task ends (no OffscreenCanvas), or false on failure.
   */
  render(f: Frame): ImageBitmap | boolean {
    const gl = this.gl;
    if (this.lost || gl.isContextLost()) return false;
    if (this.canvas.width !== f.size || this.canvas.height !== f.size) {
      this.canvas.width = f.size;
      this.canvas.height = f.size;
    }
    gl.viewport(0, 0, f.size, f.size);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    const u = this.u;
    gl.uniformMatrix3fv(u.uRot, false, f.rot);
    gl.uniform3f(u.uShift, f.shift[0], f.shift[1], f.shift[2]);
    gl.uniform4fv(u.uSpk, f.spk);
    gl.uniform4fv(u.uHW, f.hw);
    gl.uniform1i(u.uN, f.n);
    gl.uniform1f(u.uR, f.r);
    gl.uniform1f(u.uS, f.s);
    gl.uniform1f(u.uTime, f.time);
    gl.uniform1f(u.uMag, f.mag);
    gl.uniform3f(u.uMagDir, f.magDir[0], f.magDir[1], f.magDir[2]);
    gl.uniform3f(u.uAccent, f.accent[0], f.accent[1], f.accent[2]);
    gl.uniform1f(u.uDark, f.dark ? 1 : 0);
    gl.uniform1f(u.uI, f.intensity);
    gl.uniform1f(u.uB, f.burst);
    gl.uniform1f(u.uHeart, f.heart);
    gl.uniform1f(u.uWork, f.work);
    gl.drawElements(gl.TRIANGLES, this.count, gl.UNSIGNED_INT, 0);
    return this.canvas instanceof HTMLCanvasElement ? true : this.canvas.transferToImageBitmap();
  }
}

/** Read once: the 2D orb on request (to compare), or when WebGL2 doesn't exist at all. */
const FORCE_2D = (() => {
  try {
    return localStorage.getItem('democritus.orbRenderer') === '2d' || typeof WebGL2RenderingContext === 'undefined';
  } catch {
    return typeof WebGL2RenderingContext === 'undefined';
  }
})();

/** A spike slot: a direction with its tangents, its own randomness, and its liquid spring. */
interface Slot {
  dir: Vec3;
  t1: Vec3;
  t2: Vec3;
  ph: number;
  lenR: number;
  widR: number;
  holdUntil: number;
  amp: number;
  vel: number;
  /** The last target, for the respawn test. */
  target: number;
}

function randomDir(): Vec3 {
  const z = Math.random() * 2 - 1;
  const a = Math.random() * TWO;
  const r = Math.sqrt(1 - z * z);
  return [r * Math.cos(a), r * Math.sin(a), z];
}

/** The theme accent as roughly linear RGB, read from the CSS tokens. */
function accentOf(): Vec3 {
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
  const m = /^#?([0-9a-f]{6})$/i.exec(raw);
  const hex = m ? m[1] : 'b5582a';
  return [0, 2, 4].map((i) => Math.pow(parseInt(hex.slice(i, i + 2), 16) / 255, 2.2)) as Vec3;
}

export interface OrbOptions {
  /** small: 54px ring, 46px well, 44px canvas. large: up to 380px on the Training tab. */
  size: 'small' | 'large';
  tuning: OrbTuning;
  label: string;
}

export class FerroOrb {
  readonly el: HTMLElement;
  private canvas: HTMLCanvasElement;
  /** How the visible canvas is drawn: a GPU bitmap per frame, or a 2D context (copy or the 2D orb). */
  private view: { kind: 'bitmap'; ctx: ImageBitmapRenderingContext } | { kind: '2d'; ctx: CanvasRenderingContext2D } | null = null;
  private label = '';
  private readonly halo: HTMLElement;
  private readonly tuning: OrbTuning;
  private readonly large: boolean;
  private mode: OrbMode = 'calm';
  private intensity = START;
  private target = START;
  private burst = 0;
  private nextBurst = 0;
  private last = 0;
  private reduce = false;
  /** The page wants it animating (start / stop), and it is on screen. */
  private wanted = false;
  private onScreen = false;
  // 3D state
  private readonly slots: Slot[] = [];
  private flinch = 0;
  private workAcc = 0;
  private work = 0;
  private haloShown = -1;
  private magnetEase = 0;
  private pointer: { x: number; y: number } | null = null;
  private theme = '';
  private accent: Vec3 = [0.47, 0.12, 0.03];
  private readonly spk = new Float32Array(MAX_SPIKES * 4);
  private readonly hw = new Float32Array(MAX_SPIKES);
  private readonly rot = new Float32Array(9);
  private readonly started = performance.now();
  private readonly onPointer = (e: PointerEvent) => (this.pointer = { x: e.clientX, y: e.clientY });
  /** A click makes the fluid flinch (the big Training orb). */
  private readonly onFlinch = () => {
    this.flinch = Math.min(1, this.flinch + 0.8);
    this.burst = Math.max(this.burst, 0.7);
    this.workAcc += LOOK.kick;
  };
  private readonly onLeave = (e: PointerEvent) => {
    if (!e.relatedTarget) this.pointer = null; // left the window
  };

  constructor(opts: OrbOptions) {
    this.tuning = opts.tuning;
    this.large = opts.size === 'large';
    const layers = '<div class="orb-halo" aria-hidden="true"></div><div class="orb-floor" aria-hidden="true"></div><canvas></canvas>';
    if (this.large) {
      this.el = document.createElement('div');
      this.el.className = 'orb-large';
      this.el.innerHTML = layers;
    } else {
      this.el = document.createElement('div');
      this.el.className = 'orb-ring';
      this.el.innerHTML = `<div class="orb-well">${layers}</div>`;
    }
    this.canvas = this.el.querySelector('canvas')!;
    this.canvas.width = this.large ? 760 : 88;
    this.canvas.height = this.canvas.width;
    this.halo = this.el.querySelector('.orb-halo')!;
    this.canvas.setAttribute('role', 'img');
    this.setLabel(opts.label);
    // The renderer (and its mesh) is made on the first frame that is drawn.
    this.el.dataset.renderer = FORCE_2D ? '2d' : 'webgl';
    for (let k = 0; k < LOOK.spikes; k++) this.slots.push(this.spawn(performance.now(), 1500));
    if (this.large) this.canvas.addEventListener('pointerdown', this.onFlinch);
    orbOf.set(this.el, this);
    visibility.observe(this.el);
    live.add(this);
  }

  setLabel(text: string) {
    this.label = text;
    if (this.canvas.getAttribute('aria-label') !== text) this.canvas.setAttribute('aria-label', text);
  }

  /** A canvas keeps its first kind of context: switching kinds needs a new canvas. */
  private useView(kind: 'bitmap' | '2d'): CanvasRenderingContext2D | ImageBitmapRenderingContext | null {
    if (this.view?.kind === kind) return this.view.ctx;
    if (this.view) {
      const fresh = document.createElement('canvas');
      fresh.width = this.canvas.width;
      fresh.height = this.canvas.height;
      fresh.setAttribute('role', 'img');
      fresh.setAttribute('aria-label', this.label);
      this.canvas.replaceWith(fresh);
      this.canvas = fresh;
      if (this.large) fresh.addEventListener('pointerdown', this.onFlinch);
    }
    if (kind === 'bitmap') {
      const ctx = this.canvas.getContext('bitmaprenderer');
      this.view = ctx ? { kind, ctx } : null;
      return ctx;
    }
    const ctx = this.canvas.getContext('2d');
    this.view = ctx ? { kind, ctx } : null;
    return ctx;
  }

  /** Written only when it changes: a per-frame attribute write would restyle the page. */
  private setRenderer(kind: 'webgl' | '2d') {
    if (this.el.dataset.renderer !== kind) this.el.dataset.renderer = kind;
  }

  /** One unit of activity (a token, a tool step, a training step). */
  event(gain = 0.08) {
    if (this.mode !== 'active') return;
    this.target = Math.min(1, this.target + gain * (1 - this.target) * (0.6 + Math.random() * 0.8));
    // The glow follows the work: a training step (0.7) counts like several tokens (0.08).
    this.workAcc += LOOK.kick * Math.max(1, gain / 0.08);
  }

  /** Sets the state and makes sure the orb is animating. */
  setMode(mode: OrbMode) {
    if (mode !== this.mode) {
      this.mode = mode;
      if (mode === 'active') {
        this.target = Math.max(this.target, this.tuning.floor);
        this.nextBurst = 0;
      } else {
        // No work, no glow: whatever built up fades within about a second.
        this.workAcc = Math.min(this.workAcc, LOOK.kick);
      }
    }
    this.start();
  }

  /** Starts (or keeps) the animation loop; it runs while the orb can be seen. */
  start() {
    this.wanted = true;
    this.resume();
  }

  /** Called by the visibility observers. */
  setOnScreen(on: boolean) {
    this.onScreen = on;
    if (on) this.resume();
    else this.pause();
  }

  resume() {
    if (!this.wanted || !this.onScreen || document.hidden) {
      this.pause();
      return;
    }
    if (running.has(this)) {
      schedule();
      return;
    }
    try {
      this.reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
      this.reduce = false;
    }
    if (this.large) {
      window.addEventListener('pointermove', this.onPointer, { passive: true });
      document.addEventListener('pointerout', this.onLeave);
    }
    // The first frame is due at once.
    this.last = performance.now() - 1000;
    running.add(this);
    schedule();
  }

  /** When this orb's next frame is due (ms, performance.now()). */
  dueAt(): number {
    // A quiet drop needs fewer frames; a flinch or the pointer magnet stays smooth.
    const lively = this.mode === 'active' || this.flinch >= 0.02 || this.magnetEase >= 0.02;
    const fps = lively || !this.tuning.idleFps ? ACTIVE_FPS : this.tuning.idleFps;
    return this.last + 1000 / fps - 1;
  }

  /** Draws one frame (called by the shared loop). */
  frame(now: number) {
    // After a pause, start from here rather than catching up.
    if (now - this.last > 500) this.last = now - 1000 / 60;
    this.tick(now);
  }

  private pause() {
    running.delete(this);
    unschedule();
    if (this.large) {
      window.removeEventListener('pointermove', this.onPointer);
      document.removeEventListener('pointerout', this.onLeave);
    }
  }

  /** Stops the animation loop (the drop keeps its last frame) until start() again. */
  stop() {
    this.wanted = false;
    this.pause();
  }

  /** For an orb that is thrown away (its reply or task is gone). */
  dispose() {
    this.stop();
    visibility.unobserve(this.el);
    live.delete(this);
  }

  private tick(now: number) {
    const t = this.tuning;
    // Frame-rate independent: the constants are per 60 fps frame.
    const frames = Math.min(6, (now - this.last) / (1000 / 60));
    const dt = Math.min(0.033, (now - this.last) / 1000);
    this.last = now;
    const active = this.mode === 'active';
    if (active) {
      this.target = Math.max(t.floor, this.target * t.decay ** frames);
      if (this.nextBurst === 0) this.nextBurst = now + t.burstFirst * LOOK.burstEvery;
      if (now > this.nextBurst) {
        this.burst = 1;
        this.nextBurst = now + (t.burstMin + Math.random() * (t.burstMax - t.burstMin)) * LOOK.burstEvery;
      }
    } else {
      // A slow breath (about 5 s); never a burst.
      this.target = t.breathe ? t.calm * (1 - t.breatheDepth + 2 * t.breatheDepth * (0.5 + 0.5 * Math.sin((now / 5000) * TWO))) : t.calm;
    }
    this.burst *= (active ? t.burstDecayActive : t.burstDecayIdle) ** frames;
    this.intensity += (this.target - this.intensity) * (1 - (1 - t.ease) ** frames);
    this.flinch *= t.decay ** frames;
    // Glow: kicks fade with a 400 ms half-life while working (150 ms otherwise,
    // so a flinch on a quiet drop is only a flash); the glow eases at 8% per frame.
    this.workAcc *= 0.5 ** ((frames * 1000) / 60 / (active ? 400 : 150));
    this.work += (1 - Math.exp(-0.9 * this.workAcc) - this.work) * (1 - 0.92 ** frames);
    if (!this.el.isConnected) return;
    const r = this.renderer();
    if (!r || !this.draw3d(r, now, frames, dt)) {
      this.setRenderer('2d');
      this.draw(now / 1000);
    }
  }

  private renderer(): Renderer | null {
    return FORCE_2D ? null : Renderer.get(this.large ? 7 : 5);
  }

  private dark(): boolean {
    return document.documentElement.dataset.theme === 'dark';
  }

  /** A new place for spike `index`: the farthest of 8 random directions from every other slot. */
  private spawn(now: number, maxHold: number, index = -1): Slot {
    let best: Vec3 = randomDir();
    let bestScore = Infinity;
    for (let i = 0; i < 8; i++) {
      const d = randomDir();
      let worst = -1;
      this.slots.forEach((s, j) => {
        if (j !== index) worst = Math.max(worst, dot(d, s.dir));
      });
      if (worst < bestScore) {
        bestScore = worst;
        best = d;
      }
    }
    const t1 = norm(cross(best, Math.abs(best[1]) < 0.99 ? [0, 1, 0] : [1, 0, 0]));
    return {
      dir: best,
      t1,
      t2: cross(best, t1),
      ph: Math.random() * 1000,
      lenR: Math.random() * 2 - 1,
      widR: Math.random() * 2 - 1,
      holdUntil: now + (maxHold === 1500 ? Math.random() * 1500 : 80 + Math.random() * 600),
      amp: 0,
      vel: 0,
      target: 0,
    };
  }

  /** The 3D frame (Ferrofluid lab). False if WebGL couldn't draw it. */
  private draw3d(renderer: Renderer, now: number, frames: number, dt: number): boolean {
    const c = this.canvas;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const css = c.clientWidth;
    const px = Math.max(1, Math.round(css * dpr));
    if (px !== c.width) {
      c.width = px;
      c.height = px;
    }
    const dark = this.dark();
    const theme = document.documentElement.dataset.theme ?? '';
    if (theme !== this.theme) {
      this.theme = theme;
      this.accent = accentOf();
    }
    // Reduced motion: slower, calmer, no tremble or bounce.
    const rm = this.reduce;
    const speed = LOOK.speed * (rm ? 0.3 : 1);
    const real = (now - this.started) / 1000;
    const time = real * speed;
    const I = (this.intensity + this.flinch) * (rm ? 0.4 : 1);
    const B = this.burst * (rm ? 0.3 : 1);
    const bounce = rm ? 0 : LOOK.bounce;
    const tremble = rm ? 0 : LOOK.tremble;

    const hp = (time % 1.4) / 1.4;
    const beat = Math.exp(-(((hp - 0.02) * 16) ** 2)) + 0.65 * Math.exp(-(((hp - 0.2) * 16) ** 2));
    const heart = LOOK.heart * beat * (rm ? 0.3 : 1);
    const Ia = Math.min(1.2, I + heart * 0.35);

    // Rotation: a slow turn and a gentle nod (column-major mat3 Ry * Rx).
    const ay = real * (rm ? 0.3 : 1) * LOOK.rot * 0.6;
    const ax = 0.22 * Math.sin(real * (rm ? 0.3 : 1) * 0.31);
    const [cy, sy, cx, sx] = [Math.cos(ay), Math.sin(ay), Math.cos(ax), Math.sin(ax)];
    const R = this.rot;
    R.set([cy, 0, -sy, sy * sx, cx, cy * sx, sy * cx, -sx, cy * cx]);

    // Pointer magnet: the big Training orb only.
    let mc = 0;
    let magDir: Vec3 = [0, 0, 1];
    let mTarget = 0;
    let px2 = 0;
    let py2 = 0;
    const fit = Math.min(1, 0.47 / (LOOK.body * 1.12 + LOOK.length * (1 + LOOK.burstLen) * 1.05));
    if (this.large && this.pointer && css > 0) {
      const box = c.getBoundingClientRect();
      px2 = ((this.pointer.x - box.left) / box.width) * 2 - 1;
      py2 = ((this.pointer.y - box.top) / box.height) * 2 - 1;
      const radiusPx = (LOOK.body * fit) * box.height;
      const d = Math.hypot(this.pointer.x - (box.left + box.width / 2), this.pointer.y - (box.top + box.height / 2)) / radiusPx;
      mTarget = clamp(1.3 - 0.75 * d, 0, 1);
    }
    this.magnetEase += (mTarget - this.magnetEase) * (1 - 0.88 ** frames);
    if (this.large && this.magnetEase > 0.001) {
      mc = Math.min(1, LOOK.magnet * this.magnetEase);
      const w = norm([1.3 * px2, -1.3 * py2, 0.9]);
      // world → object space: transpose(uRot) * w
      magDir = norm([R[0] * w[0] + R[1] * w[1] + R[2] * w[2], R[3] * w[0] + R[4] * w[1] + R[5] * w[2], R[6] * w[0] + R[7] * w[1] + R[8] * w[2]]);
    }
    const shapeI = Math.max(Ia, LOOK.magnet * this.magnetEase * 0.75 * (this.large ? 1 : 0));
    const uR = WORLD * LOOK.body * fit * (1 - 0.12 * shapeI);
    const uS = WORLD * LOOK.length * fit * shapeI * (1 + LOOK.burstLen * B);

    // Spikes: targets, respawn, wander, magnet, and a liquid spring each.
    const N = Math.min(MAX_SPIKES, LOOK.spikes);
    const k = 220 * speed * speed;
    const damp = 2 * Math.sqrt(k) * (1 - 0.8 * bounce);
    const lo = 0.2 + 0.4 * LOOK.retract;
    const respawnP = 1 - (1 - 0.06) ** frames;
    for (let i = 0; i < N; i++) {
      let s = this.slots[i];
      if (s.target < 0.02 && s.amp < 0.03 && Math.abs(s.vel) < 0.3 && now >= s.holdUntil && Math.random() < respawnP) {
        s = this.slots[i] = this.spawn(now, 680, i);
      }
      const ph = s.ph;
      let A: number;
      if (now < s.holdUntil) {
        A = LOOK.burstAmp * B * 0.5;
      } else {
        const raw =
          0.5 +
          0.5 * (0.5 * Math.sin(time * (3.4 + 1.7 * Math.sin(ph)) + 1.7 * ph) + 0.32 * Math.sin(time * (5.3 + 2.3 * Math.cos(1.3 * ph)) + 4.1 * ph) + 0.18 * Math.sin(1.31 * time + 2.9 * ph));
        let v = clamp((raw - lo) / 0.3, 0, 1);
        for (let n = 0; n < LOOK.snap; n++) v = v * v * (3 - 2 * v);
        A = LOOK.floor + (1 - LOOK.floor) * v;
        A += (1 - A) * LOOK.burstAmp * B;
      }
      A *= 1 + LOOK.irregular * 0.4 * s.lenR;
      const w1 = LOOK.wander * (Math.sin(0.7 * time + 3.1 * ph) + 0.5 * Math.sin(1.7 * time + 5.3 * ph));
      const w2 = LOOK.wander * (Math.cos(0.53 * time + 1.9 * ph) + 0.5 * Math.cos(1.3 * time + 2.2 * ph));
      let d = norm([s.dir[0] + s.t1[0] * w1 + s.t2[0] * w2, s.dir[1] + s.t1[1] * w1 + s.t2[1] * w2, s.dir[2] + s.t1[2] * w1 + s.t2[2] * w2]);
      if (mc > 0) {
        const pull = Math.max(0, dot(d, magDir));
        A = A * (1 - 0.6 * mc * (1 - pull)) + mc * pull ** 3 * 1.1;
        d = norm([d[0] + magDir[0] * 0.35 * mc * pull, d[1] + magDir[1] * 0.35 * mc * pull, d[2] + magDir[2] * 0.35 * mc * pull]);
      }
      s.target = A;
      const h = dt / 2;
      for (let step = 0; step < 2; step++) {
        s.vel += (k * (A - s.amp) - damp * s.vel) * h;
        s.amp = clamp(s.amp + s.vel * h, -0.25, 1.8);
      }
      const amp = s.amp + tremble * 0.18 * Math.sin(31 * time + 13 * ph) * Math.sin(19.7 * time + 7 * ph) * (0.3 + Math.max(0, s.amp));
      this.spk.set([d[0], d[1], d[2], amp], i * 4);
      this.hw[i] = ((LOOK.width * 2.4) / Math.sqrt(N)) * (0.85 + 0.3 * (0.5 + 0.5 * Math.sin(3.7 * ph + 0.6 * time))) * (1 + LOOK.irregular * 0.3 * s.widR);
    }

    const shiver = tremble * 0.012 * (0.3 + Ia);
    const shift: Vec3 = [shiver * Math.sin(41 * time), shiver * Math.sin(37 * time + 1.3), 0];
    const frame = renderer.render({ size: px, rot: R, shift, spk: this.spk, hw: this.hw, n: N, r: uR, s: uS, time, mag: mc, magDir, accent: this.accent, dark, intensity: Ia, burst: B, heart, work: this.work });
    if (frame === false) return false;
    if (frame instanceof ImageBitmap) {
      const ctx = this.useView('bitmap') as ImageBitmapRenderingContext | null;
      if (!ctx) return false;
      ctx.transferFromImageBitmap(frame);
    } else {
      const ctx = this.useView('2d') as CanvasRenderingContext2D | null;
      if (!ctx) return false;
      ctx.clearRect(0, 0, px, px);
      ctx.drawImage(renderer.canvas as HTMLCanvasElement, 0, 0, px, px);
    }
    this.setRenderer('webgl');
    // Halo: dark theme only, a CSS glow behind the canvas that follows the work.
    const halo = dark ? Math.min(0.95, LOOK.halo * this.work * (1 + 0.8 * B) * 1.6) : 0;
    if (Math.abs(halo - this.haloShown) > 0.01) {
      this.haloShown = halo;
      this.halo.style.opacity = halo.toFixed(3);
    }
    return true;
  }

  /** The 2D orb of the mockups: used without WebGL2. */
  private draw(t: number) {
    const ctx = this.useView('2d') as CanvasRenderingContext2D | null;
    if (!ctx) return;
    const c = this.canvas;
    const dark = this.dark();
    // Drawn at the screen's pixel density, so the outline stays smooth at any size.
    const px = Math.round(c.clientWidth * (window.devicePixelRatio || 1));
    if (px > 0 && px !== c.width) {
      c.width = px;
      c.height = px;
    }
    const W = c.width;
    const cx = W / 2;
    const cy = this.large ? W / 2 : W / 2 - W * 0.015;
    const I = this.reduce ? this.intensity * (this.large ? 0.35 : 0.4) : this.intensity;
    const sp = this.reduce ? 0.3 : 1;
    const B = this.reduce ? this.burst * 0.3 : this.burst;
    const R = W * (this.large ? 0.175 : 0.165) * (1 - 0.12 * I);
    const S = W * (this.large ? 0.23 : 0.21) * I * (1 + (this.large ? 0.3 : 0.28) * B);
    const rot = t * 0.3 * sp;
    const hw = (Math.PI / CONES) * 0.62;
    const amps: number[] = [];
    const ths: number[] = [];
    const hws: number[] = [];
    for (let k = 0; k < CONES; k++) {
      const ph = k * 2.399;
      const n1 = Math.sin(t * (3.4 + 1.7 * Math.sin(ph)) * sp + ph * 1.7);
      const n2 = Math.sin(t * (5.3 + 2.3 * Math.cos(ph * 1.3)) * sp + ph * 4.1);
      const n3 = Math.sin(t * 1.31 * sp + ph * 2.9);
      let v = 0.5 + 0.5 * (0.5 * n1 + 0.32 * n2 + 0.18 * n3);
      v = v * v * (3 - 2 * v);
      v = v * v * (3 - 2 * v);
      let A = 0.1 + 0.9 * v;
      A = A + (1 - A) * 0.8 * B;
      amps.push(A);
      ths.push((k * TWO) / CONES + rot + hw * 0.5 * Math.sin(t * 1.1 * sp + ph * 5.1));
      hws.push(hw * (0.68 + 0.6 * (0.5 + 0.5 * Math.sin(ph * 3.7 + t * 0.6 * sp))));
    }
    ctx.clearRect(0, 0, W, W);
    const reach = R + S + W * 0.03;

    if (dark) {
      // magnetic halo that brightens with activity
      const halo = ctx.createRadialGradient(cx, cy, R * 0.6, cx, cy, reach + W * 0.09);
      halo.addColorStop(0, `rgba(232,121,58,${Math.min(0.9, 0.34 * I * (1 + 1.4 * B)).toFixed(3)})`);
      halo.addColorStop(0.5, `rgba(232,121,58,${Math.min(0.6, 0.12 * I * (1 + 1.4 * B)).toFixed(3)})`);
      halo.addColorStop(1, 'rgba(232,121,58,0)');
      ctx.fillStyle = halo;
      ctx.beginPath();
      ctx.arc(cx, cy, reach + W * 0.09, 0, TWO);
      ctx.fill();
    } else {
      // soft floor shadow
      ctx.save();
      ctx.filter = `blur(${Math.round(W * 0.034)}px)`;
      ctx.fillStyle = 'rgba(0,0,0,0.16)';
      ctx.beginPath();
      ctx.ellipse(cx, cy + R + W * 0.14, R * 1.05, W * 0.04, 0, 0, TWO);
      ctx.fill();
      ctx.restore();
    }

    // body: a wobbling, slightly sagging drop with individually breathing cones
    const points = 960;
    // Quiet drops (intensity under ~0.2) get rounded bumps instead of sharp tips;
    // every lively state (all floors are 0.22 or more) keeps the cone profile.
    const sharp = Math.min(1, Math.max(0, (this.intensity - 0.1) / 0.1));
    const path = new Path2D();
    for (let i = 0; i <= points; i++) {
      const a = (i / points) * TWO;
      let r = R * (1 + 0.055 * Math.sin(2 * a + t * 1.9 * sp) + 0.038 * Math.sin(3 * a - t * 3.0 * sp) + 0.022 * Math.sin(5 * a + t * 4.6 * sp));
      for (let k = 0; k < CONES; k++) {
        let d = a - ths[k];
        d = Math.atan2(Math.sin(d), Math.cos(d));
        // Quiet bumps are up to twice as wide and a little lower, so they read as soft swells.
        const w = hws[k] * (2 - sharp);
        if (d > -w && d < w) {
          const u = Math.cos(((d / w) * Math.PI) / 2);
          const cone = 0.28 * Math.pow(u, 1.1) + 0.72 * Math.pow(u, 5);
          const bump = 0.7 * u * u;
          r += S * amps[k] * (bump + (cone - bump) * sharp);
        }
      }
      const x = cx + r * Math.cos(a);
      const y = cy + r * Math.sin(a) * 1.04;
      if (i === 0) path.moveTo(x, y);
      else path.lineTo(x, y);
    }
    path.closePath();
    const g = ctx.createRadialGradient(cx - R * 0.4, cy - R * 0.5, this.large ? R * 0.04 : 2, cx, cy, this.large ? reach : R + S);
    g.addColorStop(0, '#8e8e8e');
    g.addColorStop(0.16, '#3c3c3c');
    g.addColorStop(0.42, '#141414');
    g.addColorStop(1, '#030303');
    ctx.fillStyle = g;
    ctx.fill(path);

    // wet highlights (and on dark backgrounds the warm rim light)
    ctx.save();
    ctx.clip(path);
    if (dark) {
      ctx.filter = `blur(${Math.round(W * 0.011)}px)`;
      ctx.fillStyle = 'rgba(255,255,255,0.55)';
      ctx.beginPath();
      ctx.ellipse(cx - R * 0.36, cy - R * 0.5, R * 0.26, R * 0.11, -0.6, 0, TWO);
      ctx.fill();
      ctx.fillStyle = 'rgba(255,255,255,0.2)';
      ctx.beginPath();
      ctx.ellipse(cx - R * 0.08, cy - R * 0.78, R * 0.55, R * 0.07, -0.22, 0, TWO);
      ctx.fill();
      ctx.filter = `blur(${Math.round(W * 0.02)}px)`;
      ctx.fillStyle = `rgba(232,121,58,${Math.min(1, 0.62 * I + 0.14 + 0.25 * B).toFixed(3)})`;
      ctx.beginPath();
      ctx.ellipse(cx + R * 0.55, cy + R * 0.62, R * 0.6, R * 0.22, -0.5, 0, TWO);
      ctx.fill();
    } else {
      ctx.filter = `blur(${Math.round(W * 0.022)}px)`;
      ctx.fillStyle = 'rgba(255,255,255,0.5)';
      ctx.beginPath();
      ctx.ellipse(cx - R * 0.33, cy - R * 0.45, R * 0.3, R * 0.13, -0.6, 0, TWO);
      ctx.fill();
      ctx.fillStyle = 'rgba(255,255,255,0.12)';
      ctx.beginPath();
      ctx.ellipse(cx + R * 0.35, cy + R * 0.5, R * 0.35, R * 0.12, -0.5, 0, TWO);
      ctx.fill();
    }
    ctx.restore();
    ctx.strokeStyle = 'rgba(255,255,255,0.07)';
    ctx.lineWidth = 2;
    ctx.stroke(path);
  }
}
