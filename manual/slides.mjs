// A note with `slides: true` in its frontmatter is a deck: every `---` (an <hr>) starts a new <section class="slide">.
// The note still reads top to bottom at /n/<id>; /s/<id> presents it one section per screen.
export default function rehypeSlides() {
  return (tree, file) => {
    if (!file.data.astro?.frontmatter?.slides) return;
    const slides = [[]];
    for (const c of tree.children) c.tagName === 'hr' ? slides.push([]) : slides.at(-1).push(c);
    tree.children = slides.map(children => ({ type: 'element', tagName: 'section', properties: { className: ['slide'] }, children }));
  };
}
