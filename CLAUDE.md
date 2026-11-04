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

## Commit rule

When a requested change is complete and its relevant checks pass, commit only the files for that change on the current feature branch and push the branch to `origin`, unless the user explicitly asks to keep it local. Follow the commit subject style in `AGENTS.md`. Never commit directly to `main`, force-push, or include generated screenshots, another agent's work, or the paused #5 stash without reviewing them. State which checks ran; leave live MCP confirmation to the verification agent.
