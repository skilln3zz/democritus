// The ferrofluid thinking orb (design-neumorphic/NEU-SPEC.md): a glossy black
// drop beside the reply being generated. While the model thinks, its spikes
// follow the rate of incoming thinking tokens; when the answer starts they melt
// into a calm, gently wobbling drop. Shape and shading are the mockup's draw()
// in design-neumorphic/mockups/NeuChat.dc.html.

/** Per-frame (60 fps) decay of the target intensity, and its floor while thinking. */
const DECAY = 0.962;
const FLOOR = 0.22;
/** Per-frame easing of the drawn intensity toward the target. */
const EASE = 0.09;
/** Intensity of the calm drop (waiting for the first token, or answering). */
const CALM = 0.05;
/**
 * How much one thinking token raises the target, as a share of the room left
 * below 1. With the decay above, a slow 27B (about 6 tokens/s) stays near the
 * floor, the 9B (about 80/s) is lively and the 4B (about 115/s) nearly maxed.
 */
const GAIN = 0.08;

export type OrbPhase = 'waiting' | 'thinking' | 'answering';

const LABELS: Record<OrbPhase, string> = {
  waiting: 'Democritus is getting ready',
  thinking: 'Democritus is thinking',
  answering: 'Democritus is answering',
};

export class Orb {
  readonly el: HTMLElement;
  private readonly canvas: HTMLCanvasElement;
  private phase: OrbPhase = 'waiting';
  private intensity = CALM;
  private target = CALM;
  private raf = 0;
  private last = 0;
  private reduce = false;

  constructor() {
    this.el = document.createElement('div');
    this.el.className = 'orb-ring';
    this.el.innerHTML = '<div class="orb-well"><canvas width="88" height="88" role="img"></canvas></div>';
    this.canvas = this.el.querySelector('canvas')!;
    this.canvas.setAttribute('aria-label', LABELS.waiting);
  }

  /** One incoming thinking token. */
  token() {
    if (this.phase !== 'thinking') this.setPhase('thinking');
    this.target = Math.min(1, this.target + GAIN * (1 - this.target) * (0.6 + Math.random() * 0.8));
  }

  setPhase(phase: OrbPhase) {
    if (phase === this.phase) return;
    this.phase = phase;
    if (phase === 'thinking') this.target = Math.max(this.target, FLOOR);
    this.canvas.setAttribute('aria-label', LABELS[phase]);
    this.start();
  }

  start() {
    if (this.raf) return;
    try {
      this.reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
      this.reduce = false;
    }
    this.last = performance.now();
    const loop = (now: number) => {
      this.tick(now);
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  /** Stops the animation loop; nothing runs while nothing is generating. */
  stop() {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
  }

  private tick(now: number) {
    // Frame-rate independent: the constants are per 60 fps frame.
    const frames = Math.min(6, (now - this.last) / (1000 / 60));
    this.last = now;
    if (this.phase === 'thinking') this.target = Math.max(FLOOR, this.target * DECAY ** frames);
    else this.target = CALM;
    this.intensity += (this.target - this.intensity) * (1 - (1 - EASE) ** frames);
    if (!this.el.isConnected) return; // rendered again later; keep the state
    this.draw(now / 1000);
  }

  private draw(t: number) {
    const c = this.canvas;
    const ctx = c.getContext('2d');
    if (!ctx) return;
    const W = c.width,
      H = c.height,
      cx = W / 2,
      cy = H / 2 - W * 0.015;
    const I = this.reduce ? this.intensity * 0.4 : this.intensity;
    const sp = this.reduce ? 0.3 : 1;
    const R = W * 0.212 * (1 - 0.1 * I),
      S = W * 0.174 * I,
      N = 15,
      rot = t * 0.22 * sp;
    ctx.clearRect(0, 0, W, H);
    ctx.save();
    ctx.filter = 'blur(' + Math.round(W * 0.034) + 'px)';
    ctx.fillStyle = 'rgba(0,0,0,0.16)';
    ctx.beginPath();
    ctx.ellipse(cx, cy + R + W * 0.1, R * 1.05, W * 0.04, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
    const path = new Path2D();
    for (let i = 0; i <= 360; i++) {
      const a = (i / 360) * Math.PI * 2;
      const base = R * (1 + 0.04 * Math.sin(3 * a + t * 1.1 * sp) + 0.028 * Math.sin(5 * a - t * 1.7 * sp));
      const spike = Math.pow(Math.abs(Math.cos((N * a) / 2 + rot)), 7) * (0.72 + 0.28 * Math.sin(4 * a + t * 2.3 * sp));
      const r = base + S * spike;
      const x = cx + r * Math.cos(a),
        y = cy + r * Math.sin(a);
      if (i === 0) path.moveTo(x, y);
      else path.lineTo(x, y);
    }
    path.closePath();
    const g = ctx.createRadialGradient(cx - R * 0.35, cy - R * 0.45, 2, cx, cy, R + S);
    g.addColorStop(0, '#4b4b4b');
    g.addColorStop(0.32, '#151515');
    g.addColorStop(1, '#020202');
    ctx.fillStyle = g;
    ctx.fill(path);
    ctx.save();
    ctx.clip(path);
    ctx.filter = 'blur(' + Math.round(W * 0.022) + 'px)';
    ctx.fillStyle = 'rgba(255,255,255,0.5)';
    ctx.beginPath();
    ctx.ellipse(cx - R * 0.33, cy - R * 0.42, R * 0.3, R * 0.15, -0.6, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = 'rgba(255,255,255,0.12)';
    ctx.beginPath();
    ctx.ellipse(cx + R * 0.35, cy + R * 0.45, R * 0.35, R * 0.12, -0.5, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
    ctx.strokeStyle = 'rgba(255,255,255,0.07)';
    ctx.lineWidth = 2;
    ctx.stroke(path);
  }
}
