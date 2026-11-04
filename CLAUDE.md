@AGENTS.md
@BUILD_AND_DEPLOY.md

## Local build and tests

Run tests from `/home/bella/Downloads/paper/sketch` (not the older checkout under `~/.local/src/sketch`):

```bash
cd /home/bella/Downloads/paper/sketch
cargo fmt --check
cargo test --all-targets -- --test-threads=1
cargo test --doc
```

Serial execution avoids a crash seen when headless GPU screenshot tests ran concurrently. Tests may regenerate `docs/screenshots/`; check `git status` and do not discard another agent's work. Build and launch the desktop app as described in `BUILD_AND_DEPLOY.md`.
