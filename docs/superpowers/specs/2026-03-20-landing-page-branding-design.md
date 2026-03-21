# Cook Landing Page & Branding Design

## Overview

Design spec for the `getcook.sh` landing page — a pre-launch teaser and vision board for Cook, the build system.

## Domain Strategy

| Domain | Purpose |
|--------|---------|
| `getcook.sh` | Developer front door — landing page, install script, SSH TUI |
| `usecook.com` | Cloud product — dashboard, billing, team management |

`getcook.sh` serves three protocols:

- **Browser** (`https://getcook.sh`) — landing page
- **curl** (`curl -sSf https://getcook.sh | bash`) — CLI install script
- **SSH** (`ssh getcook.sh`) — keyboard-driven TUI dashboard (future)

## Brand Voice

**Tone:** Minimal with a wink. Clean developer tool aesthetic with the cooking metaphor surfacing at exactly three strategic moments — not decorating every surface.

**Metaphor usage points:**
1. Hero headline: "Something is in the Oven."
2. Features header: "Our Kitchen. Your Code."
3. Roadmap header: "The Full Menu."

Status badges extend the metaphor naturally: "Live" / "In the Oven" / "On the Menu."

The rest of the page is straight developer-tool language — no forced puns.

## Visual Direction

**Aesthetic:** Dark base, Turborepo-style structure, warm fire accents.

**Color palette:**
- Background: `#0a0a0a` (near-black)
- Text: `#fafafa` (primary), `#888`/`#666`/`#555` (secondary hierarchy)
- Accent gradient: `#f97316` (orange) → `#ef4444` (red) — the fire/oven signature
- Highlight: `#fbbf24` (amber, for code strings and warm moments)
- Success: `#22c55e` (green, for "Live" status badges)

**Typography:**
- Headlines: Inter, 900 weight, tight letter-spacing (-2 to -3.5px)
- Body: Inter, 400/500 weight
- Code: JetBrains Mono

**Signature visual:** Animated fire glow behind the hero headline. CSS-approximated with layered radial gradients and flicker animations (blur: 70px, opacity: 0.45). Production version should use a WebGL fragment shader for richer effect.

**Logo:** Orange-to-red gradient rounded square with a pan (🍳) icon + "cook" wordmark in 800 weight.

## Page Structure (The Showcase)

### 1. Fixed Nav
- Logo (pan icon + "cook")
- Links: Docs, Cloud, Examples
- GitHub star button + "Join Waitlist" CTA

### 2. Hero — "Something is in the Oven."
- Animated fire glow behind headline
- Pulsing "Coming Soon" badge
- Sub: "A build system you can actually read. Readable recipes, blazing caching, and a cloud that scales with your team."
- Primary CTA: email waitlist form
- Secondary: `curl -sSf https://getcook.sh | bash` install command

### 3. "Read It and Weep." — Syntax Comparison
- Side-by-side Makefile vs Cookfile
- Makefile is dimmed/muted, Cookfile is highlighted with orange border
- Drives the readability story viscerally
- Tagline: "Same build. A fraction of the ceremony."

### 4. "Our Kitchen. Your Code." — Feature Grid
6 cards in a 3-column grid:
- Parallel Recipes — DAG-based scheduling, zero config
- Smart Caching — hash-based with mtime fast-path, free forever
- File Watching — `cook serve` built in
- Cloud Cache — shared team artifacts
- Lua Scripting — embedded scripting when shell isn't enough
- Cloud Dashboard — build status, analytics, cache hit rates

### 5. "The Full Menu." — Roadmap / Vision Board
Living roadmap with status badges:

| Item | Status |
|------|--------|
| Cook CLI | Live |
| Local Cache | Live |
| Cloud Cache | In the Oven |
| Web Dashboard | In the Oven |
| SSH Terminal UI | On the Menu |
| Remote Builds | On the Menu |
| CI/CD Integration | On the Menu |

Update badges as features ship. This section is the personal vision board.

### 6. SSH Teaser
Terminal-style block showing `$ ssh getcook.sh` with "Your builds. A keyboard away."

### 7. Final CTA — "Ready to start cooking?"
"Free and open-source. Cloud when your team is ready." + second email waitlist form.

### 8. Footer
Logo, GitHub/Docs/Twitter links, `getcook.sh` domain.

## Dual Purpose

This page serves two audiences:
1. **Visitors:** Get the hook immediately, understand what Cook is, join the waitlist
2. **Alex (creator):** A vision board showing the full roadmap with progress indicators, a source of motivation and direction

## Mockup Reference

Art direction mockup saved at: `marketing/.superpowers/brainstorm/` (showcase-v1.html)

The mockup establishes visual direction. Code examples in the mockup are approximate — replace with real Cookfile syntax when building the production page.

## Tech Decisions (Deferred)

- Static site framework (Astro, Next.js, plain HTML)
- WebGL shader library for fire effect
- Email waitlist backend (Resend, Buttondown, etc.)
- SSH TUI framework (Charm/Wish)

These are implementation decisions for the planning phase.
