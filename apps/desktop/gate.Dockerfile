# The system libraries the desktop shell needs, and nothing else.
#
# The shell is its own workspace, so the crate gate never needs these; this
# image exists for the one job that does. Build it once and reuse it:
#
#   docker build -t flit-desktop-gate -f apps/desktop/gate.Dockerfile apps/desktop
#   docker run --rm -v "$PWD:/src" -w /src/apps/desktop/src-tauri \
#     flit-desktop-gate cargo clippy --all-targets -- -D warnings
#
# webkit2gtk is the webview, libappindicator is the tray icon, librsvg draws the
# bundle icons and libxdo is what the window's drag region needs on X11.
FROM rust:1.99-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libwebkit2gtk-4.1-dev \
      libayatana-appindicator3-dev \
      librsvg2-dev \
      libxdo-dev \
      libssl-dev \
 && rm -rf /var/lib/apt/lists/*
