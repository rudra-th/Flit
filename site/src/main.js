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
const wide = window.matchMedia("(min-width: 1041px)");

/* --- the reel ------------------------------------------------------------ */

const hero = document.querySelector(".hero");
const track = document.querySelector(".reel-track");
const frames = track ? [...track.children] : [];

let queued = 0;

function updateReel() {
  queued = 0;
  if (!hero || !track || frames.length < 2 || still.matches || !wide.matches) {
    track?.style.removeProperty("--step");
    return;
  }

  const span = hero.offsetHeight - window.innerHeight;
  const progress = span <= 0 ? 0 : (window.scrollY - hero.offsetTop) / span;
  const step = Math.round(Math.min(1, Math.max(0, progress)) * (frames.length - 1));
  track.style.setProperty("--step", String(step));
}

function requestReel() {
  if (queued) return;
  queued = requestAnimationFrame(updateReel);
}

window.addEventListener("scroll", requestReel, { passive: true });
window.addEventListener("resize", requestReel);
still.addEventListener("change", updateReel);
wide.addEventListener("change", updateReel);
updateReel();

/* --- the figure ---------------------------------------------------------
 * 105 MiB/s is the measured rate for a 100 GiB loopback send, recorded in
 * docs/benchmarks.md. The formula is one division, and the page says so.
 */

const RATE = 105;

const size = document.getElementById("size");
const sizeOut = document.getElementById("size-out");
const figureNum = document.getElementById("figure-num");

function setSize() {
  if (!size || !figureNum) return;
  const gib = Number(size.value);
  if (sizeOut) sizeOut.textContent = `${gib} GiB`;
  figureNum.textContent = String(Math.round((gib * 1024) / RATE));
}

if (size) {
  size.addEventListener("input", setSize);
  setSize();
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
