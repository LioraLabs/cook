// Ordered-dither fills for post charts: a 4×4 Bayer matrix decides which 2px cells light up,
// so a density from 0 to 1 becomes a pixel texture instead of a smooth gradient.
const BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]].map(r => r.map(v => (v + .5) / 16));
const CELL = 2; // CSS px per dither cell

/** Sizes the canvas to its box at device resolution and returns a painter for density-driven cells. */
function setup(canvas: HTMLCanvasElement) {
  const dpr = window.devicePixelRatio || 1, w = canvas.clientWidth, h = canvas.clientHeight;
  canvas.width = Math.round(w * dpr); canvas.height = Math.round(h * dpr);
  const ctx = canvas.getContext('2d')!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.fillStyle = getComputedStyle(canvas).color; // colour comes from CSS, so var(--a) etc. just work
  const dot = Math.max(1, Math.floor(1.5 * dpr)) / dpr; // snapped to device pixels so dots stay crisp
  const cell = (cx: number, cy: number, density: number) => {
    if (density > BAYER[cy & 3][cx & 3]) ctx.fillRect(cx * CELL, cy * CELL, dot, dot);
  };
  return { w, h, cols: Math.ceil(w / CELL), rows: Math.ceil(h / CELL), cell, ctx };
}

/** A bar: a translucent wash under dense dither, or diagonal crosshatch for results that are wrong. */
export function paintBar(canvas: HTMLCanvasElement, style: 'dots' | 'hatch') {
  const { w, h, cols, rows, cell, ctx } = setup(canvas);
  ctx.globalAlpha = style === 'hatch' ? .12 : .4; // a translucent wash under the dots
  ctx.fillRect(0, 0, w, h);
  ctx.globalAlpha = 1;
  for (let y = 0; y < rows; y++) for (let x = 0; x < cols; x++) {
    if (style === 'hatch') cell(x, y, (x + y) % 3 === 0 ? 1 : 0);
    else cell(x, y, .8 + .2 * Math.sin((y + .5) / rows * Math.PI));
  }
}

/** The area under a line, fading from dense at the line to nothing at the baseline. ys are 0..1 from the top. */
export function paintArea(canvas: HTMLCanvasElement, xs: number[], ys: number[], texture: 'dither' | 'hatch' = 'dither') {
  const { w, h, cols, rows, cell } = setup(canvas);
  for (let x = 0; x < cols; x++) {
    const px = ((x + .5) * CELL) / w;
    if (px < xs[0] || px > xs[xs.length - 1]) continue;
    let i = 1; while (i < xs.length - 1 && xs[i] < px) i++;
    const t = (px - xs[i - 1]) / (xs[i] - xs[i - 1] || 1), top = (ys[i - 1] + (ys[i] - ys[i - 1]) * t) * h;
    for (let y = Math.floor(top / CELL); y < rows; y++) {
      const fall = ((y * CELL) - top) / Math.max(1, h - top); // 0 at the line, 1 at the baseline
      const d = .8 * (1 - fall) ** 1.6;
      // a second series gets diagonal hatch, so overlapping areas differ by texture, not just colour
      cell(x, y, texture === 'hatch' ? ((x + y) % 4 === 0 ? d * 1.6 : 0) : d);
    }
  }
}

/** Repaints on resize; canvases are sized by CSS, so a new width means a new pixel grid. */
export function onResize(el: Element, paint: () => void) {
  paint();
  new ResizeObserver(() => paint()).observe(el);
}
