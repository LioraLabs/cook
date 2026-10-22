# Cook pipeline motion graphic

18 seconds · 1440 × 900 · 30 fps · silent H.264 MP4.

- `cook-pipeline.mp4`: rendered video.
- `index.html` + `motion.js`: editable, self-contained browser animation with local fonts.
- `fonts/`: Inter and JetBrains Mono, with their licenses.

The sequence shows lowering and registration (0–5s), DAG construction (5–9s), and cache-aware execution (9–18s). The example's six units, worker count, cache outcome, and timings are a simulation, not a measured build. One compile is cached; two compile jobs execute concurrently; link, test, and package follow declared dependencies. Playback stops on the completed build; Replay explicitly resets the timeline.

The runtime uses one deterministic `window.motionGraphic.render(seconds)` function. `?export=1` disables playback and controls. The narrow layout recomposes the explanation vertically. Reduced motion starts paused; the Play button and timeline are keyboard accessible.

## Preview and verify

From the site directory:

```sh
python3 -m http.server 4387 --bind 127.0.0.1 --directory public
# Open http://127.0.0.1:4387/graphics/cook-pipeline/

# In another terminal; omit the environment variable to use Playwright's browser.
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium node scripts/cook-motion-check.mjs
```

Export to a new filename (the helper refuses overwrites):

```sh
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium \
node ../.agents/skills/animated-systems/scripts/export.mjs \
  --url http://127.0.0.1:4387/graphics/cook-pipeline/ \
  --out /tmp/cook-pipeline.mp4 --width 1440 --height 900 --fps 30
```

## Source grounding

Reviewed against the local `~/dev/cook` implementation:

- `cli/crates/cook-luagen/README.md`: AST lowering emits register-phase Lua; luagen does not execute it.
- `cli/crates/cook-register/README.md`: `register_cookfile` captures declared units. `cook.exec` captures work during registration; `cook.sh` is a documented immediate-execution exception.
- `cli/crates/cook-plan/README.md`: workspace registration supplies the plan and declared recipe edges. Matching paths alone do not infer dependencies.
- `cli/crates/cook-engine/src/run.rs`: the engine builds one unified work-unit DAG.
- `cli/crates/cook-engine/src/dag_builder.rs`: `unit_graph::plan` wires declared dependencies before lowering to DAG nodes.
- `cli/crates/cook-dag/src/lib.rs`: `initial_ready` selects zero-dependency units; `complete` releases dependents when their remaining dependency count reaches zero.
- `cli/crates/cook-engine/src/executor.rs`: the engine checks the cache before worker dispatch.
- `cli/crates/cook-execute/README.md`: fixed worker threads each own a Lua VM and execute captured work items; shell execution goes through `cook-shell`.

The graphic abstracts probes, workspace discovery, failure propagation, cache fingerprints, and storage tiers to keep the requested luagen → DAG → execution story readable.
