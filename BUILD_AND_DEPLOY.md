# Build and link the Sketch desktop app (Linux)

Use this guide for the checkout at `/home/bella/Downloads/paper/sketch`. It installs **the desktop app**, not the MCP bridge. The app-menu entry runs `~/.local/bin/sketch`, which launches the copy at `~/.local/opt/sketch/sketch-rs`.

## Build

```bash
cd /home/bella/Downloads/paper/sketch
git status --short --branch
cargo fmt --check
cargo test --all-targets -- --test-threads=1
cargo test --doc
cargo build --release --bin sketch-rs
```

Check with anyone else editing the checkout before building from a dirty working tree. Serial tests avoid a headless GPU crash seen when screenshot tests ran concurrently. Tests may regenerate files under `docs/screenshots/`; review `git status` before discarding generated files. Do not discard another agent's work.

The Wayland touchpad pinch backport is pinned in `Cargo.toml` and `Cargo.lock`. Do not replace it with a stock winit version during a dependency update.

## Install the built app

```bash
install -d "$HOME/.local/opt/sketch"
install -m 755 target/release/sketch-rs "$HOME/.local/opt/sketch/sketch-rs.new"
cmp target/release/sketch-rs "$HOME/.local/opt/sketch/sketch-rs.new"
mv -f "$HOME/.local/opt/sketch/sketch-rs.new" "$HOME/.local/opt/sketch/sketch-rs"
sha256sum target/release/sketch-rs "$HOME/.local/opt/sketch/sketch-rs"
```

The two hashes must match. The rename replaces the installed executable atomically. A Sketch process that is already running still uses the old executable until restarted.

## Link the app to the launcher and app menu

Run this once to create the launcher and desktop entry. Later builds only need the install step above. The wrapper sets native Wayland when available; a plain symlink to the executable would not do that.

```bash
install -d "$HOME/.local/bin" "$HOME/.local/share/applications" \
  "$HOME/.local/share/icons/hicolor/scalable/apps"
cat > "$HOME/.local/bin/sketch" <<'SH'
#!/usr/bin/env bash
if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    export WINIT_UNIX_BACKEND=wayland
fi
exec "$HOME/.local/opt/sketch/sketch-rs" "$@"
SH
chmod 755 "$HOME/.local/bin/sketch"
install -m 644 site/static/favicon.svg \
  "$HOME/.local/share/icons/hicolor/scalable/apps/sketch-rs.svg"
cat > "$HOME/.local/share/applications/sketch-rs.desktop" <<EOF
[Desktop Entry]
Name=Sketch
Comment=Native Rust infinite canvas
Exec=$HOME/.local/bin/sketch %F
Icon=$HOME/.local/share/icons/hicolor/scalable/apps/sketch-rs.svg
Terminal=false
Type=Application
Categories=Graphics;
EOF
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$HOME/.local/share/applications"
fi
```

This is the launcher and `.desktop` entry currently used on this machine. Close **all** running Sketch windows, then reopen Sketch from the app menu to run the newly installed binary. You can also start it with `~/.local/bin/sketch` from a Wayland terminal. Building alone does not update a running process.

MCP bridge updates and Claude Code's cached tool schemas are separate from linking the desktop app. See `README.md` for general development and other platforms; the verification agent owns live MCP retesting and updates to `auto-ap/sketch_improvement.md`.
