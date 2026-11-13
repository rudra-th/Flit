# The Flit landing page

One static page, two hand-written source files, two self-hosted typefaces, and
no third-party request of any kind. It exists to explain Flit honestly before
there is anything to download.

```
site/
  index.html          the page: hero, three exchanges, figure, five bands, questions, get, board
  src/site.css        the app's tokens, the fragments, the layout, the motion
  src/main.js         the hero scene, the figure, the switches, the entrances, the clock
  public/fonts/       Poppins 500/600, Open Sans 400/600 + their licences
  public/favicon.svg  the two-chevron mark on jade
```

## Run it

```sh
npm install
npm run dev      # http://localhost:1421
npm run build    # writes dist/
npm run preview  # serves dist/ on 1421
```

`dist/` is what ships: ~31 kB of HTML, ~26 kB of CSS, ~5 kB of JS, plus four
woff2 files. No client framework, no router, no analytics, no cookie.

## What drives it

The page follows a direction sheet drawn with the `design-taste-frontend`
skill, saved at `.taste-skill/direction.json`:

| | |
| --- | --- |
| Seed | 1764418230 |
| Direction | The product performs |
| Hero | Sentence split by a frame |
| Type & colour | kept from the brand (the app's own tokens) |
| Sections | how it works (call and response column) → stats (figure set by a slider) → features (sentences with switches) → questions (accordion of filled tiles) → footer (a board of tiles) |
| Motion | Pinned story |

The idea the page has to land: **the interface is the picture, and the page is
a performance of it.** So the hero sentence is split by a working fragment of
the app, the "three moves" are written as calls (what you do) and responses
(what the app answers with), and the five promises are switches you can throw.

### The live parts

- **The hero scene** — one performance driven by one scroll number. A cover
  line ("Two devices, one room.") holds the first screen; scrolling fades it and
  brings "Send a folder / across the room." in from the two edges, opens a
  window between the halves, then runs three fragments (Nearby, Transfers,
  Recent) through it, holding each state for a beat before the next. It scrubs
  backwards as willingly as forwards; nothing captures the wheel. Decoration
  only, and the fragments are drawn in DOM, not screenshots.
- **The phone hero** — the same beats in one column: the cover, the two halves
  of the sentence stacked and sliding in from the sides, then the window opening
  under them and holding the same three states. A phone gets 320 svh of scene
  instead of the desk's 380, and the sentence keeps one `<h1>` box (so its line
  breaks and its accessible name survive) rather than being split around the
  frame.
- **Three moves** — on a wide screen the section holds still for 220 svh while
  each column's answer arrives in turn, its bars and the shared rail filling
  with it. Stacked, the pin is dropped and each column scrubs on its own as it
  reaches the reader. Both are scroll-*driven*: stop anywhere and the scene
  stops with you.
- **The figure** — a real slider from 1 to 100 GiB. Seconds = size × 1024 ÷ 105,
  the measured 100 GiB loopback rate from `docs/benchmarks.md`, and the page
  says the formula next to the number.
- **The five switches** — on shows the mechanism that keeps the promise. Two are
  on at load (bands 1 and 4) so the section arrives with its colour field
  visible. They are `<button role="switch">` with `aria-checked`, not checkboxes.
- **The questions** — `<details>` tiles, exclusive while the browser supports
  `name`, first one open.
- **The clock** — the visitor's own local time and `Intl` time zone.
- **Entrances** — parts of each section arrive from just below as they enter the
  viewport, capped at a 240 ms stagger. With no script, or with reduced motion,
  the page is simply the finished page.

## Redraws, and why

Each of these departs from the direction sheet or from the app, and each has a
reason:

1. **Photographs → interface fragments.** The sheet offers an "Architecture"
   photo set as atmosphere. Redrawn: this product's pictures are its own
   interface, and a stock photograph would break both the zero-third-party rule
   and the nothing-invented rule. The four fragments carry the picture count.
2. **The 4-emoji identity badge → the 4-word half only.** The app shows four
   emoji and four words for fingerprint checking. Emoji in page text is a hard
   gate failure, and words alone are the more useful half for a page you cannot
   compare against another screen.
3. **Monospace addresses → the text face with tabular figures.** The app sets
   `192.168.1.31` and `89 MB/s` in a mono face; on a marketing page mono outside
   `<code>` is a gate failure. Two faces only: Poppins for display, Open Sans for
   everything else, `font-variant-numeric: tabular-nums` so rates align.
4. **The phone keeps one sentence box, above the window.** The blueprint wants
   the strip between the halves of an `<h1>` at every width. On a phone that
   would break the heading's box, its line breaks and its accessible name, so
   the two halves stack above the frame as one `<h1>` and slide in from the
   sides while the frame opens below them. Between 861 and 1040 px the scene
   drops altogether (the sentence is simply read beside the frames), and a short
   landscape phone keeps that static version too.
5. **The bands are a value ladder, not five colours.** Off is the page ground and
   `--ink-2`. On fills with one step of the accent's own scale (five steps, one
   hue), with the mechanism text appearing under the sentence.
6. **The city/time tile became the visitor's local time.** There is no office to
   print, and inventing one would be a lie. The tile shows the reader's own clock
   and zone instead.
7. **The contact tile is a link to GitHub issues.** No address exists to print;
   `github.com/rudra-th/Flit/issues` is real and reaches a human.
8. **The end of the funnel says "nothing to download yet".** Flit is pre-alpha
   and the installer is not built. A disabled download button or a waitlist would
   be furniture; the page states the position and links the two things that do
   exist: the source and the releases page.
9. **The licence line is `MIT OR Apache-2.0`, not GPL.** The repository is dual
   licensed (`LICENSE-MIT`, `LICENSE-APACHE`); the page says what the repository
   says.
10. **Speed numbers are only the measured ones.** 105 MiB/s for a 100 GiB send
    over loopback, 139 MiB/s for 1 GiB, 15.6 MiB peak RSS in
    `docs/benchmarks.md`. A gigabit cable tops out near 118 MiB/s, so the page
    frames the network as the limit rather than the engine.
11. **The open question is a deeper jade.** White on the button's `#0e7a5c` is
    5.3:1; the open tile takes `#0a6350`, so its answer clears 7:1 without adding
    a hue. It also keeps the filled tile distinct from the page's buttons.
12. **The speed figure is one panel, not an open field.** The blueprint puts the
    control under the question on the page ground. Here question, control (top
    right) and figure (bottom left) share one white panel, so the section reads
    as one instrument: set a size, watch the figure answer. The slider fills its
    track up to the thumb, so the size is legible from the track alone.

## Copy deck

- Hero: *Send a folder across the room.* — "No account, no cable, no cloud. Flit
  finds the other device on your network, pins its key, and pushes the files
  straight across."
- Three moves: *First, drop what you want to move.* / *Then choose the screen in
  front of you.* / *Last, it lands under its real name.*
- The figure: *How big is the file?* — "Size ÷ 105 MiB/s, the rate measured on a
  100 GiB send over loopback. A gigabit cable tops out near 118 MiB/s; Wi-Fi is
  usually a fraction of it." The slider runs 1–100 GiB; the label reads the
  value in GiB.
- Five promises: *What it never does* — never asks for an account · never gives a
  broken file its name · never trusts a key it has not pinned · never repeats what
  already arrived · never needs the internet.
- Questions: eight, from "Do I need an account?" to "Is it free?"
- End: *Nothing to download yet.*

Demo data used in the fragments (the same batch throughout): Studio PC /
Rhea – Pixel 9 / Attic NAS, "Q3 handover" 15 files · 186.4 MB, 89 MB/s, 2.1 s,
transfer header "2 running · 113 MB/s" (89 + 24).

## The gate

Run before committing, against the production build:

```sh
npm run build
npm run preview -- --port 1422 &
node ~/.agents/skills/design-taste-frontend/scripts/inspect.mjs \
  http://localhost:1422/ --out .taste-skill/inspect-final \
  --widths 1440,768,390 --film
```

`--film` photographs the scroll a third of a screen at a time with real wheel
input, so the hero and the pinned "Three moves" are judged on their in-between
states, not just their rest states.

Current result: **0 FAIL, 4 WARN**.

| Warning | Why it stands |
| --- | --- |
| `palette-cream` | `#f6f5f2` is the app's own ground token, not a decorative cream; the palette is kept from the brand by direction. |
| `size-count` | 16 sizes because the interface fragments are true to the app's UI scale (12–14 px) while the page runs on a five-step display scale. |
| `no-footage` | No clip fits and none may be licensed: the subject's world is the interface, and the fragments are drawn in DOM. |
| `long-pin` | The hero sentence and the "Three moves" heading each hold for four screens, but every screen changes what is under them: the cover hands to the sentence, the window opens, three states pass through, then three columns answer in turn. That is the rule's own remedy, not a defence. |

## House rules this page obeys

- One accent (`#0e7a5c`), one ground, three radii (16 / 11 / 999), no shadows,
  no gradients, no glass, no mono outside `<code>`, no emoji in text.
- Nothing invented: no testimonials, no logos, no press, no user counts, no
  download, no email address, no city.
- Every claim traceable to the repository: the protocol, the resume behaviour,
  the pinning rule, the licences, the benchmark numbers.
