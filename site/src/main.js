// The page's three jobs, in one small file:
//
//  1. the hero reel: the sentence holds while three frames of the app pass
//     through it, one per screen of scroll (decoration, so it fails well);
//  2. the speed figure: the visitor's own folder size, through a stated rate;
//  3. the rest of the motion layer: entrances, the band switches, the clock.
//
// Everything here is optional. With no script the page is the finished page:
// the first frame of the reel, the sentence, every answer.

const still = window.matchMedia("(prefers-reduced-motion: reduce)");
// The hero runs on the wide stage and on the phone; the middle widths keep the
// plain stacked opening, where the sentence already sits beside the frames.
const wide = window.matchMedia(
  "(min-width: 1041px), (max-width: 860px) and (min-height: 600px)",
);
const phone = window.matchMedia("(max-width: 860px)");

/* --- the hero ------------------------------------------------------------
 * One performance, driven by one number: how far the reader has scrolled
 * through the stage. The cover leaves, the sentence arrives from both edges,
 * the frame opens between the halves, then three states of the app run through
 * it. Scroll-*driven*, not scroll-jacked: nothing here captures the wheel, so
 * the reader can stop mid-sentence and scrub it back.
 *
 *   0.00–0.04  the cover, held
 *   0.04–0.17  the cover leaves upward
 *   0.10–0.28  the sentence arrives from the two edges
 *   0.19–0.33  the frame opens from a hairline
 *   0.33–0.93  three frames, one continuous pass
 */

const hero = document.querySelector(".hero");
const track = document.querySelector(".reel-track");
const frames = track ? [...track.children] : [];

const clamp01 = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);
const seg = (v, a, b) => clamp01((v - a) / (b - a));
const outCubic = (t) => 1 - Math.pow(1 - t, 3);
const inOutCubic = (t) =>
  t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;

let heroTop = 0;
let heroSpan = 0;
let frame = 0;

function measureHero() {
  if (!hero) return;
  heroTop = hero.getBoundingClientRect().top + window.scrollY;
  heroSpan = hero.offsetHeight - window.innerHeight;
}

function updateHero() {
  if (!hero || !track || frames.length < 2) return;

  if (!wide.matches || still.matches) {
    hero.style.removeProperty("--intro");
    hero.style.removeProperty("--split");
    hero.style.removeProperty("--open");
    hero.classList.remove("is-intro-gone");
    track.style.removeProperty("--step");
    for (const frame of frames) frame.style.removeProperty("--near");
    return;
  }

  const p = heroSpan <= 0 ? 0 : clamp01((window.scrollY - heroTop) / heroSpan);
  // The reel holds each state for a beat and then moves to the next, like a
  // strip being pulled one frame at a time — rather than resting on a seam
  // between two frames.
  const raw = seg(p, 0.33, 0.93) * (frames.length - 1);
  const held = Math.min(Math.floor(raw), frames.length - 2);
  const step = held + outCubic(clamp01((raw - held - 0.18) / 0.64));
  // Stacked, the cover has to be gone before the sentence takes its place, or
  // the two lines sit on top of each other: the phone hands over a little
  // earlier and the sentence follows straight after.
  const intro = 1 - inOutCubic(seg(p, 0.02, phone.matches ? 0.1 : 0.17));
  const split = outCubic(seg(p, phone.matches ? 0.06 : 0.1, phone.matches ? 0.22 : 0.28));

  hero.style.setProperty("--intro", String(intro));
  hero.classList.toggle("is-intro-gone", intro < 0.01);
  hero.style.setProperty("--split", String(split));
  hero.style.setProperty("--open", String(inOutCubic(seg(p, 0.19, 0.33))));
  track.style.setProperty("--step", step.toFixed(4));

  // The frame in the window is full size and opaque; the ones on their way
  // through are smaller and dimmer, so the reel reads as depth, not a list.
  for (let i = 0; i < frames.length; i += 1) {
    const near = String(clamp01(1 - Math.abs(i - step) * 1.15).toFixed(3));
    frames[i].style.setProperty("--near", near);
  }
}

/* --- the three moves ------------------------------------------------------
 * The same idea one screen further down: the section holds while its three
 * answers arrive, in order. One scroll number (0 → 1 across the hold) becomes
 * three steps; each step lifts its column's answer into place and fills that
 * segment of the rail. Also scroll-*driven*: the reader sets the pace, stops
 * where they like, and scrolls back to replay it.
 */

const how = document.querySelector(".how");
const howWide = window.matchMedia("(min-width: 1121px)");
const moves = [...document.querySelectorAll(".how .exchange")];
const rail = [...document.querySelectorAll(".how-rail span")];

let howTop = 0;
let howSpan = 0;

function measureHow() {
  if (!how) return;
  howTop = how.getBoundingClientRect().top + window.scrollY;
  howSpan = how.offsetHeight - window.innerHeight;
}

function updateHow() {
  if (!how || !moves.length) return;

  if (still.matches) {
    for (const move of moves) move.style.removeProperty("--s");
    for (const seg of rail) seg.style.removeProperty("--f");
    return;
  }

  if (!howWide.matches) {
    // Stacked, each column answers as it reaches the reader: the top of a
    // move is at 92% of the screen when its scrub starts, and 42% when it is
    // done. Its segment of the rail fills with it. A column still more than a
    // screen below the reader is simply answered (it is nowhere near the
    // screen), so the page never holds a hidden hole below the fold.
    const H = window.innerHeight;
    for (let i = 0; i < moves.length; i += 1) {
      const r = moves[i].getBoundingClientRect();
      const s =
        r.top > H * 1.1
          ? 1
          : outCubic(clamp01((H * 0.92 - r.top) / (H * 0.5)));
      moves[i].style.setProperty("--s", s.toFixed(3));
      if (rail[i]) rail[i].style.setProperty("--f", s.toFixed(3));
    }
    return;
  }

  const p = howSpan <= 0 ? 0 : clamp01((window.scrollY - howTop) / howSpan);
  const t = p * moves.length;

  for (let i = 0; i < moves.length; i += 1) {
    const s = outCubic(seg(t, i, i + 0.85));
    moves[i].style.setProperty("--s", s.toFixed(3));
    if (rail[i]) rail[i].style.setProperty("--f", s.toFixed(3));
  }
}

/* One frame for everything driven by scroll. Both stages read the same scroll
 * position, so they must land in the same paint. */
function tick() {
  frame = 0;
  updateHero();
  updateHow();
}

function requestFrame() {
  if (frame) return;
  frame = requestAnimationFrame(tick);
}

function measure() {
  measureHero();
  measureHow();
}

window.addEventListener("scroll", requestFrame, { passive: true });
window.addEventListener("resize", () => {
  measure();
  requestFrame();
});
still.addEventListener("change", requestFrame);
wide.addEventListener("change", requestFrame);
howWide.addEventListener("change", requestFrame);
measure();
tick();

/* --- the figure ---------------------------------------------------------
 * 105 MiB/s is the measured rate for a 100 GiB loopback send, recorded in
 * docs/benchmarks.md. The formula is one division, and the page says so.
 */

const RATE = 105;

const size = document.getElementById("size");
const sizeOut = document.getElementById("size-out");
const figureNum = document.getElementById("figure-num");
const figureEl = document.querySelector(".stats-figure");

let figureJob = 0;

function setSize() {
  if (!size || !figureNum) return;
  // A slider input takes the number back from any arrival still running.
  if (figureJob) {
    cancelAnimationFrame(figureJob);
    figureJob = 0;
  }
  const gib = Number(size.value);
  const min = Number(size.min) || 0;
  const max = Number(size.max) || 100;
  // the track fills to the thumb, so the control shows where the reader is
  size.style.setProperty("--v", String((gib - min) / (max - min)));
  if (sizeOut) sizeOut.textContent = `${gib} GiB`;
  figureNum.textContent = String(Math.round((gib * 1024) / RATE));
}

if (size) {
  size.addEventListener("input", setSize);
  setSize();
}

/* The figure climbs to its value the first time it is seen. It is the same
 * number the slider gives — the animation is arrival, not arithmetic. */
if (figureEl && figureNum && !still.matches && "IntersectionObserver" in window) {
  const climb = () => {
    if (figureJob) return;
    const target = Number(figureNum.textContent.replace(/\D/g, "")) || 0;
    const live = figureNum.parentElement;
    const start = performance.now();
    const run = (now) => {
      const t = clamp01((now - start) / 720);
      figureNum.textContent = String(Math.round(target * outCubic(t)));
      figureJob = t < 1 ? requestAnimationFrame(run) : 0;
      if (t < 1) return;
      // One announcement at the end, not sixty on the way there.
      live.setAttribute("aria-live", "polite");
    };
    live.setAttribute("aria-live", "off");
    figureNum.textContent = "0";
    figureJob = requestAnimationFrame(run);
  };

  const once = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        once.disconnect();
        climb();
      }
    },
    { threshold: 0.4 },
  );
  once.observe(figureEl);
}

/* --- the bands ----------------------------------------------------------- */

for (const band of document.querySelectorAll(".band")) {
  const toggle = band.querySelector(".band-switch");
  if (!toggle) continue;
  const on = band.dataset.on === "true";
  band.dataset.on = String(on);
  toggle.addEventListener("click", () => {
    const next = band.dataset.on !== "true";
    band.dataset.on = String(next);
    toggle.setAttribute("aria-checked", String(next));
  });
}

/* --- entrances ----------------------------------------------------------- */

const reveals = [...document.querySelectorAll("[data-reveal]")];

if (reveals.length) {
  if (still.matches || !("IntersectionObserver" in window)) {
    for (const el of reveals) el.classList.add("is-in");
  } else {
    // Stagger by position within a group, so a block arrives as parts rather
    // than as one fade. Capped: no element waits longer than a quarter second.
    const seen = new Map();
    for (const el of reveals) {
      const parent = el.parentElement;
      const index = seen.get(parent) ?? 0;
      el.style.setProperty("--d", `${Math.min(index * 60, 240)}ms`);
      seen.set(parent, index + 1);
    }

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          entry.target.classList.add("is-in");
          observer.unobserve(entry.target);
        }
      },
      { rootMargin: "0px 0px -10% 0px", threshold: 0.05 },
    );

    for (const el of reveals) observer.observe(el);
  }
}

/* --- the clock ----------------------------------------------------------
 * The footer gives the visitor's own time and zone. There is no office to
 * put in this tile, so it does not pretend there is one.
 */

const clock = document.getElementById("clock");
const zone = document.getElementById("zone");

if (clock) {
  const time = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  const tick = () => {
    clock.textContent = time.format(new Date());
  };
  tick();
  window.setInterval(tick, 15_000);

  if (zone) {
    const name = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (name) zone.textContent = name.replace(/_/g, " ");
  }
}

/* The page is now in charge of its own opening. Until this line runs the inline
 * fallback in <head> is counting two seconds and will drop back to the still
 * page if the module never arrives. */
window.__flitReady = true;
