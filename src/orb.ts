// The ferrofluid orb (design-neumorphic/NEU-SPEC.md): a glossy black drop with
// 11 cones. Built once and used in three places: small beside the reply being
// generated in chat, small beside the coding agent's reply, and large on the
// Training tab. Shape and shading are the mockups' draw(): NeuCode.dc.html for
// light backgrounds, NeuTrain.dc.html (halo and warm rim light) for dark ones.
//
// What drives it is up to the page: each event() (a thinking token, a tool
// step, a training step) raises a target intensity, which decays every frame
// and is eased toward (over about a second). Two modes:
//   active  cones up, follows the events, bursts every few seconds
//   calm    a quiet drop breathing slowly between 0.08 and 0.12 with soft bumps
//           (chat while getting ready and answering, the agent while it waits for
//           you or is done, the Training tab while idle or paused); no bursts
//
// An orb is never completely still while it can be seen. Its loop pauses only
// while it can't be: the window is hidden or minimised, or the orb is off screen
// (another page or view, or scrolled out of sight). The quiet drop is drawn at
// 30 fps; everything is drawn at the screen's devicePixelRatio.

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

export interface OrbOptions {
  /** small: 54px ring, 46px well, 44px canvas drawn at 2×. large: 380px canvas. */
  size: 'small' | 'large';
  tuning: OrbTuning;
  label: string;
}

export class FerroOrb {
  readonly el: HTMLElement;
  private readonly canvas: HTMLCanvasElement;
  private readonly tuning: OrbTuning;
  private readonly large: boolean;
  private mode: OrbMode = 'calm';
  private intensity = START;
  private target = START;
  private burst = 0;
  private nextBurst = 0;
  private raf = 0;
  private last = 0;
  private reduce = false;
  /** The page wants it animating (start / stop), and it is on screen. */
  private wanted = false;
  private onScreen = false;

  constructor(opts: OrbOptions) {
    this.tuning = opts.tuning;
    this.large = opts.size === 'large';
    if (this.large) {
      this.el = document.createElement('canvas');
      this.el.className = 'orb-large';
      this.canvas = this.el as HTMLCanvasElement;
      this.canvas.width = 760;
      this.canvas.height = 760;
    } else {
      this.el = document.createElement('div');
      this.el.className = 'orb-ring';
      this.el.innerHTML = '<div class="orb-well"><canvas width="88" height="88"></canvas></div>';
      this.canvas = this.el.querySelector('canvas')!;
    }
    this.canvas.setAttribute('role', 'img');
    this.setLabel(opts.label);
    orbOf.set(this.el, this);
    visibility.observe(this.el);
    live.add(this);
  }

  setLabel(text: string) {
    if (this.canvas.getAttribute('aria-label') !== text) this.canvas.setAttribute('aria-label', text);
  }

  /** One unit of activity (a token, a tool step, a training step). */
  event(gain = 0.08) {
    if (this.mode !== 'active') return;
    this.target = Math.min(1, this.target + gain * (1 - this.target) * (0.6 + Math.random() * 0.8));
  }

  /** Sets the state and makes sure the orb is animating. */
  setMode(mode: OrbMode) {
    if (mode !== this.mode) {
      this.mode = mode;
      if (mode === 'active') {
        this.target = Math.max(this.target, this.tuning.floor);
        this.nextBurst = 0;
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
    if (this.raf) return;
    try {
      this.reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
      this.reduce = false;
    }
    this.last = performance.now();
    const loop = (now: number) => {
      this.raf = 0;
      // A quiet drop needs fewer frames: skip until the frame-rate cap allows the next one.
      const cap = this.tuning.idleFps;
      if (cap && this.mode !== 'active' && now - this.last < 1000 / cap - 2) {
        this.raf = requestAnimationFrame(loop);
        return;
      }
      this.tick(now);
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  private pause() {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
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
    this.last = now;
    const active = this.mode === 'active';
    if (active) {
      this.target = Math.max(t.floor, this.target * t.decay ** frames);
      if (this.nextBurst === 0) this.nextBurst = now + t.burstFirst;
      if (now > this.nextBurst) {
        this.burst = 1;
        this.nextBurst = now + t.burstMin + Math.random() * (t.burstMax - t.burstMin);
      }
    } else {
      // A slow breath (about 5 s); never a burst.
      this.target = t.breathe ? t.calm * (1 - t.breatheDepth + 2 * t.breatheDepth * (0.5 + 0.5 * Math.sin((now / 5000) * TWO))) : t.calm;
    }
    this.burst *= (active ? t.burstDecayActive : t.burstDecayIdle) ** frames;
    this.intensity += (this.target - this.intensity) * (1 - (1 - t.ease) ** frames);
    if (this.el.isConnected) this.draw(now / 1000);
  }

  private dark(): boolean {
    return document.documentElement.dataset.theme === 'dark';
  }

  private draw(t: number) {
    const c = this.canvas;
    const ctx = c.getContext('2d');
    if (!ctx) return;
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
