# Cook Landing Page Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `getcook.sh` pre-launch landing page — a dark, fire-accented teaser page with waitlist signup, real Cookfile examples, feature grid, and living roadmap.

**Architecture:** Single-page static site built with Vite (vanilla JS, no framework). WebGL fragment shader for the hero fire effect. Formspree for zero-backend email waitlist collection. Deploys as static files to any CDN (Cloudflare Pages, Netlify, etc).

**Tech Stack:** Vite 6, vanilla HTML/CSS/JS, WebGL 2 (raw — no Three.js), Formspree (waitlist), Inter + JetBrains Mono (Google Fonts)

**Spec:** `docs/superpowers/specs/2026-03-20-landing-page-branding-design.md`
**Art direction mockup:** `marketing/.superpowers/brainstorm/822195-1774023034/showcase-v1.html`

---

## File Structure

```
marketing/
├── index.html              # Page structure, all sections
├── style.css               # Full stylesheet — theme, layout, components
├── main.js                 # Entry point — initializes shader, form handling
├── fire-shader.js          # WebGL fire shader setup and render loop
├── shaders/
│   ├── fire.vert           # Vertex shader (fullscreen quad)
│   └── fire.frag           # Fragment shader (noise-based fire)
├── package.json            # Vite dev dependency
├── vite.config.js          # Vite config (shader imports)
├── public/
│   └── favicon.svg         # Orange-red gradient pan icon
└── .gitignore              # node_modules, dist
```

---

## Chunk 1: Foundation

### Task 1: Project Scaffolding

**Files:**
- Create: `marketing/package.json`
- Create: `marketing/vite.config.js`
- Create: `marketing/.gitignore`
- Create: `marketing/index.html` (skeleton)
- Create: `marketing/main.js` (empty entry)
- Create: `marketing/style.css` (empty)

- [ ] **Step 1: Initialize package.json**

```json
{
  "name": "cook-landing",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview"
  }
}
```

- [ ] **Step 2: Install Vite**

Run: `cd marketing && npm install --save-dev vite`

- [ ] **Step 3: Create vite.config.js**

```js
import { defineConfig } from "vite";

export default defineConfig({
  assetsInclude: ["**/*.vert", "**/*.frag"],
});
```

- [ ] **Step 4: Create .gitignore**

```
node_modules
dist
.DS_Store
```

- [ ] **Step 5: Create skeleton index.html**

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Cook — A build system you can actually read</title>
  <meta name="description" content="Readable recipes, blazing caching, and a cloud that scales with your team. Something is in the oven." />
  <link rel="icon" href="/favicon.svg" />
  <link rel="preconnect" href="https://fonts.googleapis.com" />
  <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700;800;900&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet" />
  <link rel="stylesheet" href="/style.css" />
</head>
<body>
  <div id="app"></div>
  <script type="module" src="/main.js"></script>
</body>
</html>
```

- [ ] **Step 6: Create empty main.js and style.css**

`main.js`:
```js
import "./style.css";
console.log("cook landing loaded");
```

`style.css`: empty file.

- [ ] **Step 7: Verify dev server starts**

Run: `cd marketing && npm run dev`
Expected: Vite dev server starts on localhost, blank page loads with console message.

- [ ] **Step 8: Create favicon.svg**

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#f97316"/>
      <stop offset="100%" stop-color="#ef4444"/>
    </linearGradient>
  </defs>
  <rect width="32" height="32" rx="7" fill="url(#g)"/>
  <text x="16" y="22" text-anchor="middle" font-size="18">🍳</text>
</svg>
```

Save to `marketing/public/favicon.svg`.

- [ ] **Step 9: Commit**

```bash
git add marketing/package.json marketing/package-lock.json marketing/vite.config.js marketing/.gitignore marketing/index.html marketing/main.js marketing/style.css marketing/public/favicon.svg
git commit -m "feat(marketing): scaffold Vite project for landing page"
```

---

### Task 2: HTML Structure — All Sections

**Files:**
- Modify: `marketing/index.html`

Build the complete HTML structure inside `<body>`. No styling yet — just semantic markup. Reference the spec for section order and content. Use real Cookfile syntax from the repo examples.

- [ ] **Step 1: Write the full HTML structure**

Replace the `<body>` contents of `index.html` with all sections. The page structure is:

```html
<body>
  <!-- Nav -->
  <nav class="nav">
    <div class="nav-logo">
      <img src="/favicon.svg" alt="Cook" class="nav-logo-icon" />
      <span>cook</span>
    </div>
    <div class="nav-links">
      <a href="https://github.com/Alex-Gilbert/cook#readme">Docs</a>
      <a href="#features">Cloud</a>
      <a href="https://github.com/Alex-Gilbert/cook/tree/main/examples">Examples</a>
    </div>
    <div class="nav-right">
      <a href="https://github.com/Alex-Gilbert/cook" class="gh-star">
        <svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>
        Star
      </a>
      <a href="#waitlist" class="nav-cta">Join Waitlist</a>
    </div>
  </nav>

  <!-- Hero -->
  <section class="hero" id="hero">
    <canvas id="fire-canvas"></canvas>
    <div class="hero-content">
      <div class="hero-badge">
        <span class="badge-dot"></span>
        Coming Soon
      </div>
      <h1>Something is<br />in the Oven.</h1>
      <p class="hero-sub">
        A build system you can actually <strong>read</strong>.
        Readable recipes, blazing caching, and a cloud
        that scales with your team.
      </p>
      <form class="waitlist" id="waitlist" action="https://formspree.io/f/FORM_ID" method="POST">
        <input type="email" name="email" placeholder="you@company.com" required />
        <button type="submit">Join the Waitlist</button>
      </form>
      <div class="hero-or">or try it now</div>
      <div class="hero-install">
        <span class="install-dollar">$</span>
        <code>curl -sSf https://getcook.sh | bash</code>
      </div>
    </div>
  </section>

  <div class="divider"></div>

  <!-- Syntax Comparison -->
  <section class="syntax" id="syntax">
    <div class="syntax-header">
      <h2>Read It and Weep.</h2>
      <p>No <code>$@</code> <code>$&lt;</code> <code>$^</code> cryptography. Just say what you mean.</p>
    </div>
    <div class="code-compare">
      <div class="code-block code-block--dim">
        <div class="code-header">
          <span class="code-dot code-dot--red"></span>
          <span class="code-dot code-dot--yellow"></span>
          <span class="code-dot code-dot--green"></span>
          <span class="code-filename">Makefile</span>
          <span class="code-label">before</span>
        </div>
        <pre class="code-body"><span class="tok-dim">CC = gcc
CFLAGS = -Wall -Wextra
SRCS = $(wildcard src/*.c)
OBJS = $(SRCS:.c=.o)

build: $(OBJS)
&#9;$(CC) $(LDFLAGS) -o <span class="tok-var">$@</span> <span class="tok-var">$^</span>

%.o: %.c
&#9;$(CC) $(CFLAGS) -c <span class="tok-var">$&lt;</span> -o <span class="tok-var">$@</span>

clean:
&#9;rm -f $(OBJS) build</span></pre>
      </div>
      <div class="code-block code-block--highlight">
        <div class="code-header">
          <span class="code-dot code-dot--red"></span>
          <span class="code-dot code-dot--yellow"></span>
          <span class="code-dot code-dot--green"></span>
          <span class="code-filename">Cookfile</span>
          <span class="code-label code-label--accent">after</span>
        </div>
        <pre class="code-body"><span class="tok-cmt"># Same build. A fraction of the ceremony.</span>

<span class="tok-kw">CC</span> <span class="tok-str">"gcc"</span>
<span class="tok-kw">CFLAGS</span> <span class="tok-str">"-Wall -Wextra"</span>

<span class="tok-kw">recipe</span> <span class="tok-str">"lib"</span>
    <span class="tok-kw">ingredients</span> <span class="tok-str">"lib/*.c"</span> <span class="tok-str">"include/*.h"</span>
    <span class="tok-kw">cook</span> <span class="tok-str">"build/obj/{stem}.o"</span> <span class="tok-kw">using</span> <span class="tok-str">"{CC} {CFLAGS} -Iinclude -c {in} -o {out}"</span>
    <span class="tok-kw">cook</span> <span class="tok-str">"build/libmath.a"</span> <span class="tok-kw">using</span> <span class="tok-str">"{AR} rcs {out} {all}"</span>
<span class="tok-kw">end</span>

<span class="tok-kw">recipe</span> <span class="tok-str">"build"</span>: <span class="tok-str">"lib"</span>
    <span class="tok-kw">ingredients</span> <span class="tok-str">"src/*.c"</span>
    <span class="tok-kw">cook</span> <span class="tok-str">"bin/app"</span> <span class="tok-kw">using</span> <span class="tok-str">"{CC} {CFLAGS} {all} -Iinclude -Lbuild -lmath -lm -o {out}"</span>
<span class="tok-kw">end</span></pre>
      </div>
    </div>
  </section>

  <div class="divider"></div>

  <!-- Features -->
  <section class="features" id="features">
    <div class="features-header">
      <h2>Our Kitchen. Your Code.</h2>
      <p>Everything you need to build fast and ship faster.</p>
    </div>
    <div class="feature-grid">
      <div class="feature-card">
        <div class="feature-icon">⚡</div>
        <h3>Parallel Recipes</h3>
        <p>DAG-based scheduling runs independent recipes simultaneously. Zero config.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">📦</div>
        <h3>Smart Caching</h3>
        <p>Hash-based caching with mtime fast-path. Never rebuild what hasn't changed. Free, forever.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">👀</div>
        <h3>File Watching</h3>
        <p><code>cook serve</code> — watches your ingredients and re-runs on change. Built in.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">☁️</div>
        <h3>Cloud Cache</h3>
        <p>Share cached artifacts across your entire team. One person builds, everyone benefits.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">🧑‍💻</div>
        <h3>Lua Scripting</h3>
        <p>Embedded Lua for when shell isn't enough. Filesystem API, path helpers, full exec control.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">📊</div>
        <h3>Cloud Dashboard</h3>
        <p>Build status, timing analytics, cache hit rates. Everything your team ships, visible.</p>
      </div>
    </div>
  </section>

  <div class="divider"></div>

  <!-- Roadmap -->
  <section class="roadmap" id="roadmap">
    <div class="roadmap-header">
      <h2>The Full Menu.</h2>
      <p>What's cooking and what's coming.</p>
    </div>
    <div class="roadmap-list">
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--live">Live</span>
        <div class="roadmap-info">
          <h3>Cook CLI</h3>
          <p>Readable Cookfile syntax, parallel execution, smart caching, file watching, Lua scripting. Free and open-source.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--live">Live</span>
        <div class="roadmap-info">
          <h3>Local Cache</h3>
          <p>Hash-based incremental caching with mtime fast-path. Deterministic, automatic, zero-config.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--oven">In the Oven</span>
        <div class="roadmap-info">
          <h3>Cloud Cache</h3>
          <p>Shared artifact cache across your team. Build once, everyone benefits.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--oven">In the Oven</span>
        <div class="roadmap-info">
          <h3>Web Dashboard</h3>
          <p>Build status, logs, timing analytics, cache hit rates. The command center for your team's builds.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--menu">On the Menu</span>
        <div class="roadmap-info">
          <h3>SSH Terminal UI</h3>
          <p><code>ssh getcook.sh</code> — a keyboard-driven TUI for your builds. No browser needed.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--menu">On the Menu</span>
        <div class="roadmap-info">
          <h3>Remote Builds</h3>
          <p><code>cook build --cloud</code> — offload builds to managed workers. Scale-to-zero.</p>
        </div>
      </div>
      <div class="roadmap-item">
        <span class="roadmap-status roadmap-status--menu">On the Menu</span>
        <div class="roadmap-info">
          <h3>CI/CD Integration</h3>
          <p>GitHub Actions, GitLab CI, and more. Managed build execution in your existing pipelines.</p>
        </div>
      </div>
    </div>
  </section>

  <!-- SSH Teaser -->
  <section class="ssh-teaser">
    <div class="ssh-block">
      <div class="ssh-prompt">$ <span>ssh getcook.sh</span></div>
      <div class="ssh-response">connecting...<br />Welcome to Cook Cloud. <span class="ssh-cursor">█</span></div>
    </div>
    <p class="ssh-caption">Your builds. A keyboard away.</p>
  </section>

  <!-- Final CTA -->
  <section class="final-cta" id="final-cta">
    <h2>Ready to start cooking?</h2>
    <p>Free and open-source. Cloud when your team is ready.</p>
    <form class="waitlist" action="https://formspree.io/f/FORM_ID" method="POST">
      <input type="email" name="email" placeholder="you@company.com" required />
      <button type="submit">Join the Waitlist</button>
    </form>
  </section>

  <!-- Footer -->
  <footer class="footer">
    <div class="footer-logo">
      <img src="/favicon.svg" alt="Cook" width="22" height="22" />
      <span>cook</span>
    </div>
    <div class="footer-links">
      <a href="https://github.com/Alex-Gilbert/cook">GitHub</a>
      <a href="https://github.com/Alex-Gilbert/cook#readme">Docs</a>
    </div>
    <div class="footer-domain">getcook.sh</div>
  </footer>

  <script type="module" src="/main.js"></script>
</body>
```

- [ ] **Step 2: Verify page loads with raw unstyled HTML**

Run: `cd marketing && npm run dev`
Expected: All sections visible as unstyled HTML. Links work. Forms present.

- [ ] **Step 3: Commit**

```bash
git add marketing/index.html
git commit -m "feat(marketing): add complete HTML structure for landing page"
```

---

### Task 3: CSS — Base Theme and Layout

**Files:**
- Create: `marketing/style.css`

Implement the full stylesheet matching the art direction mockup. Color palette, typography, and all component styles.

Reference the mockup at `marketing/.superpowers/brainstorm/822195-1774023034/showcase-v1.html` for exact values. Key design tokens:

- Background: `#0a0a0a`
- Text primary: `#fafafa`, secondary: `#888`, `#666`, `#555`
- Accent gradient: `#f97316` → `#ef4444`
- Amber highlight: `#fbbf24`
- Success green: `#22c55e`
- Headlines: Inter, weight 900, letter-spacing -2px to -3.5px
- Code: JetBrains Mono
- Border: `rgba(255,255,255,0.06)` standard, `rgba(255,255,255,0.1)` interactive

- [ ] **Step 1: Write the complete stylesheet**

Port all CSS from the showcase-v1.html mockup into `style.css`. The mockup contains the complete, validated styles for every component. Copy them, organized into these sections:

1. **Reset & base** — box-sizing, body background/color/font
2. **Nav** — fixed, backdrop-blur, logo, links, CTA button
3. **Hero** — centered layout, fire canvas positioning, badge, h1, subtitle, waitlist form, install command
4. **Divider** — gradient horizontal rule
5. **Syntax section** — header, code-compare grid, code blocks (dim vs highlight), syntax token colors
6. **Features** — header, 3-column grid, feature cards with icon, hover states
7. **Roadmap** — list layout, status badges (live/oven/menu), item rows
8. **SSH teaser** — centered terminal block, cursor blink animation
9. **Final CTA** — centered, waitlist form repeat
10. **Footer** — flex layout, links, domain

Key additions beyond the mockup:
- Add `scroll-behavior: smooth` to html
- Add `cursor: blink` animation for the SSH cursor: `@keyframes blink { 0%,100% { opacity:1; } 50% { opacity:0; } }`
- The `#fire-canvas` should be `position: absolute; top: 0; left: 0; width: 100%; height: 100%; z-index: 0;` within the hero
- Responsive breakpoints (Task 6 handles detail, but set up the container max-widths now)

- [ ] **Step 2: Verify in browser**

Run: `cd marketing && npm run dev`
Expected: Page matches the art direction mockup — dark theme, correct typography, all sections styled. Fire canvas area is blank (shader not implemented yet).

- [ ] **Step 3: Commit**

```bash
git add marketing/style.css
git commit -m "feat(marketing): add complete landing page styles"
```

---

### Task 4: WebGL Fire Shader

**Files:**
- Create: `marketing/shaders/fire.vert`
- Create: `marketing/shaders/fire.frag`
- Create: `marketing/fire-shader.js`
- Modify: `marketing/main.js`

Implement a subtle, animated fire glow effect behind the hero headline. The effect should be:
- Warm orange/amber/red tones
- Soft, blurred, organic movement (like embers or a hearth glow)
- Subtle — enhances the text, doesn't overpower it
- Blended with the dark background via low opacity

- [ ] **Step 1: Create vertex shader**

`marketing/shaders/fire.vert` — a fullscreen quad vertex shader:

```glsl
#version 300 es
in vec2 a_position;
out vec2 v_uv;

void main() {
    v_uv = a_position * 0.5 + 0.5;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
```

- [ ] **Step 2: Create fragment shader**

`marketing/shaders/fire.frag` — noise-based fire effect:

```glsl
#version 300 es
precision highp float;

in vec2 v_uv;
out vec4 fragColor;

uniform float u_time;
uniform vec2 u_resolution;

// Simplex-style noise helpers
vec3 mod289(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec2 mod289(vec2 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec3 permute(vec3 x) { return mod289(((x * 34.0) + 1.0) * x); }

float snoise(vec2 v) {
    const vec4 C = vec4(
        0.211324865405187,   // (3.0-sqrt(3.0))/6.0
        0.366025403784439,   // 0.5*(sqrt(3.0)-1.0)
       -0.577350269189626,   // -1.0 + 2.0 * C.x
        0.024390243902439    // 1.0 / 41.0
    );
    vec2 i = floor(v + dot(v, C.yy));
    vec2 x0 = v - i + dot(i, C.xx);
    vec2 i1 = (x0.x > x0.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
    vec4 x12 = x0.xyxy + C.xxzz;
    x12.xy -= i1;
    i = mod289(i);
    vec3 p = permute(permute(i.y + vec3(0.0, i1.y, 1.0)) + i.x + vec3(0.0, i1.x, 1.0));
    vec3 m = max(0.5 - vec3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.0);
    m = m * m;
    m = m * m;
    vec3 x = 2.0 * fract(p * C.www) - 1.0;
    vec3 h = abs(x) - 0.5;
    vec3 ox = floor(x + 0.5);
    vec3 a0 = x - ox;
    m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
    vec3 g;
    g.x = a0.x * x0.x + h.x * x0.y;
    g.yz = a0.yz * x12.xz + h.yz * x12.yw;
    return 130.0 * dot(m, g);
}

float fbm(vec2 p) {
    float value = 0.0;
    float amplitude = 0.5;
    for (int i = 0; i < 5; i++) {
        value += amplitude * snoise(p);
        p *= 2.0;
        amplitude *= 0.5;
    }
    return value;
}

void main() {
    vec2 uv = v_uv;
    vec2 aspect = vec2(u_resolution.x / u_resolution.y, 1.0);

    // Center the effect and scale
    vec2 p = (uv - 0.5) * aspect;

    // Upward drift for fire movement
    float t = u_time * 0.3;
    vec2 drift = vec2(0.0, -t);

    // Layered noise for organic fire
    float n1 = fbm(p * 3.0 + drift);
    float n2 = fbm(p * 5.0 + drift * 1.3 + 3.14);
    float n3 = fbm(p * 2.0 + drift * 0.7 + 6.28);

    float fire = n1 * 0.5 + n2 * 0.3 + n3 * 0.2;

    // Shape: fade toward edges and top
    float vignette = 1.0 - length(p * vec2(1.2, 0.8));
    vignette = smoothstep(0.0, 0.7, vignette);

    // Fade toward bottom (fire rises)
    float rise = smoothstep(-0.1, 0.4, -p.y + fire * 0.3);

    float intensity = fire * vignette * rise;
    intensity = smoothstep(0.0, 0.6, intensity);

    // Fire color gradient: dark red -> orange -> amber
    vec3 col1 = vec3(0.6, 0.1, 0.0);   // deep red
    vec3 col2 = vec3(0.976, 0.451, 0.086); // #f97316 orange
    vec3 col3 = vec3(0.984, 0.749, 0.141); // #fbbf24 amber

    vec3 color = mix(col1, col2, smoothstep(0.0, 0.5, intensity));
    color = mix(color, col3, smoothstep(0.5, 1.0, intensity));

    // Subtle overall opacity
    float alpha = intensity * 0.4;

    fragColor = vec4(color, alpha);
}
```

- [ ] **Step 3: Create fire-shader.js**

`marketing/fire-shader.js` — WebGL setup and render loop:

```js
import fireVert from "./shaders/fire.vert?raw";
import fireFrag from "./shaders/fire.frag?raw";

export function initFireShader(canvas) {
  const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: false });
  if (!gl) {
    console.warn("WebGL2 not available, skipping fire shader");
    return null;
  }

  // Compile shader
  function compile(type, source) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      console.error(gl.getShaderInfoLog(shader));
      gl.deleteShader(shader);
      return null;
    }
    return shader;
  }

  const vert = compile(gl.VERTEX_SHADER, fireVert);
  const frag = compile(gl.FRAGMENT_SHADER, fireFrag);
  if (!vert || !frag) return null;

  const program = gl.createProgram();
  gl.attachShader(program, vert);
  gl.attachShader(program, frag);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.error(gl.getProgramInfoLog(program));
    return null;
  }

  // Fullscreen quad
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
    gl.STATIC_DRAW
  );

  const aPos = gl.getAttribLocation(program, "a_position");
  const uTime = gl.getUniformLocation(program, "u_time");
  const uRes = gl.getUniformLocation(program, "u_resolution");

  // Resize handler
  function resize() {
    const dpr = Math.min(window.devicePixelRatio, 2);
    const w = canvas.clientWidth * dpr;
    const h = canvas.clientHeight * dpr;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
  }

  // Render loop
  let animId;
  const startTime = performance.now();

  function render() {
    resize();
    gl.viewport(0, 0, canvas.width, canvas.height);

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);

    gl.useProgram(program);
    gl.uniform1f(uTime, (performance.now() - startTime) / 1000);
    gl.uniform2f(uRes, canvas.width, canvas.height);

    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.enableVertexAttribArray(aPos);
    gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

    gl.drawArrays(gl.TRIANGLES, 0, 6);
    animId = requestAnimationFrame(render);
  }

  render();

  // Return cleanup function
  return () => cancelAnimationFrame(animId);
}
```

- [ ] **Step 4: Wire up main.js**

```js
import "./style.css";
import { initFireShader } from "./fire-shader.js";

// Fire shader
const canvas = document.getElementById("fire-canvas");
if (canvas) {
  initFireShader(canvas);
}
```

- [ ] **Step 5: Verify fire shader renders**

Run: `cd marketing && npm run dev`
Expected: Animated warm fire glow visible behind the hero text. Subtle, organic movement. Orange/amber/red tones. Should not overpower the text.

- [ ] **Step 6: Tune shader parameters if needed**

Adjust these values in `fire.frag` if the effect is too strong or too weak:
- `alpha = intensity * 0.4` — overall opacity (lower = more subtle)
- `p * 3.0` in n1 — noise scale (higher = smaller flames)
- `u_time * 0.3` — speed of drift (lower = slower)

- [ ] **Step 7: Commit**

```bash
git add marketing/shaders/ marketing/fire-shader.js marketing/main.js
git commit -m "feat(marketing): add WebGL fire shader for hero background"
```

---

## Chunk 2: Content Polish

### Task 5: Responsive Design

**Files:**
- Modify: `marketing/style.css`

Add media queries for tablet (≤768px) and mobile (≤480px).

- [ ] **Step 1: Add responsive breakpoints**

Add to bottom of `style.css`:

```css
/* ─── Tablet ─── */
@media (max-width: 768px) {
  .nav { padding: 12px 24px; }
  .nav-links { display: none; }
  .hero h1 { font-size: 48px; letter-spacing: -2px; }
  .hero-sub { font-size: 16px; }
  .waitlist { flex-direction: column; align-items: center; }
  .waitlist input { width: 100%; max-width: 320px; }
  .waitlist button { width: 100%; max-width: 320px; }
  .code-compare { grid-template-columns: 1fr; }
  .feature-grid { grid-template-columns: 1fr 1fr; }
  .syntax-header h2,
  .features-header h2,
  .roadmap-header h2 { font-size: 32px; }
}

/* ─── Mobile ─── */
@media (max-width: 480px) {
  .nav { padding: 12px 16px; }
  .gh-star { display: none; }
  .hero { padding: 120px 16px 80px; }
  .hero h1 { font-size: 36px; letter-spacing: -1.5px; }
  .hero-install { font-size: 11px; padding: 8px 12px; }
  .feature-grid { grid-template-columns: 1fr; }
  .code-body { font-size: 11px; padding: 12px; }
  .roadmap-item { flex-direction: column; gap: 8px; }
  .roadmap-status { align-self: flex-start; }
  .final-cta h2 { font-size: 28px; }
}
```

- [ ] **Step 2: Test at multiple widths**

Run: `cd marketing && npm run dev`
Test at: 1440px (desktop), 768px (tablet), 375px (mobile).
Expected: Readable, usable at all sizes. No horizontal scroll. Code blocks scroll horizontally if needed.

- [ ] **Step 3: Commit**

```bash
git add marketing/style.css
git commit -m "feat(marketing): add responsive design breakpoints"
```

---

### Task 6: Waitlist Form Handling

**Files:**
- Modify: `marketing/main.js`

Add client-side form handling with success/error states. The form `action` points to Formspree — Alex will need to create a Formspree account and replace `FORM_ID` in the HTML. For now, implement the client-side UX.

- [ ] **Step 1: Add form submission handler to main.js**

Append to `main.js`:

```js
// Waitlist form handling
document.querySelectorAll(".waitlist").forEach((form) => {
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const input = form.querySelector("input[type=email]");
    const button = form.querySelector("button");
    const email = input.value;

    button.textContent = "Sending...";
    button.disabled = true;

    try {
      const res = await fetch(form.action, {
        method: "POST",
        headers: { "Content-Type": "application/json", Accept: "application/json" },
        body: JSON.stringify({ email }),
      });

      if (res.ok) {
        button.textContent = "You're in! 🍳";
        input.value = "";
        input.disabled = true;
      } else {
        throw new Error("Form submission failed");
      }
    } catch {
      button.textContent = "Try again";
      button.disabled = false;
    }
  });
});
```

- [ ] **Step 2: Add smooth scroll for nav anchor links**

Append to `main.js`:

```js
// Smooth scroll for anchor links
document.querySelectorAll('a[href^="#"]').forEach((link) => {
  link.addEventListener("click", (e) => {
    const target = document.querySelector(link.getAttribute("href"));
    if (target) {
      e.preventDefault();
      target.scrollIntoView({ behavior: "smooth" });
    }
  });
});
```

- [ ] **Step 3: Verify form UX**

Run: `cd marketing && npm run dev`
Expected: Clicking "Join the Waitlist" shows "Sending..." state. Without a real Formspree ID it will fail gracefully showing "Try again". With a real ID it shows "You're in! 🍳".

- [ ] **Step 4: Commit**

```bash
git add marketing/main.js
git commit -m "feat(marketing): add waitlist form handling and smooth scroll"
```

---

## Chunk 3: Build & Deploy

### Task 7: Production Build & Deploy Config

**Files:**
- Modify: `marketing/package.json`
- Create: `marketing/public/_headers` (optional, for Cloudflare Pages)

- [ ] **Step 1: Verify production build**

Run: `cd marketing && npm run build`
Expected: `dist/` directory created with minified HTML, CSS, JS, and shader files bundled.

- [ ] **Step 2: Preview production build**

Run: `cd marketing && npm run preview`
Expected: Production build serves correctly at localhost. Fire shader works. All styles present.

- [ ] **Step 3: Add Cloudflare Pages headers (optional)**

Create `marketing/public/_headers`:

```
/*
  X-Frame-Options: DENY
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin

/assets/*
  Cache-Control: public, max-age=31536000, immutable
```

- [ ] **Step 4: Commit**

```bash
git add marketing/
git commit -m "feat(marketing): add production build config"
```

- [ ] **Step 5: Update index.html with real Formspree ID**

When Alex creates a Formspree form, replace both instances of `https://formspree.io/f/FORM_ID` in `index.html` with the real endpoint. This is a manual step — just a find-and-replace.

---

## Notes for the implementor

- **Art direction reference:** The validated mockup is at `marketing/.superpowers/brainstorm/822195-1774023034/showcase-v1.html`. Open it in a browser to see the target. Port styles from there — don't reinvent.
- **Shader tuning:** The fire shader values in this plan are a starting point. Tune visually — the goal is a subtle warm glow, not a blazing inferno.
- **Formspree:** Requires Alex to create a free account at formspree.io and create a form. Until then, the form will gracefully fail.
- **Code examples:** The Makefile vs Cookfile comparison uses real syntax from `examples/Cookfile`. The Cookfile side shows the C math library build which demonstrates `ingredients`, `cook`, and dependency syntax.
- **Deployment:** Run `npm run build` in `marketing/`, deploy the `dist/` folder to any static host. Cloudflare Pages, Netlify, or Vercel all work — just point the build directory to `marketing/dist`.
