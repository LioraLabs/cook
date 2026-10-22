// The code-block theme: the trailer's Kanagawa (Wave) palette, so docs code reads like the videos.
const c = { bg: '#1F1F28', ink: '#DCD7BA', muted: '#C8C093', faint: '#727169', violet: '#957FB8', green: '#98BB6C', yellow: '#E6C384', orange: '#FFA066', blue: '#7FB4CA', aqua: '#7AA89F', pink: '#D27E99', red: '#E46876', line: '#363646' };

export default {
  name: 'kanagawa',
  type: 'dark',
  colors: {
    'editor.background': c.bg,
    'editor.foreground': c.ink,
    'editor.selectionBackground': '#2D4F67',
    'editorLineNumber.foreground': c.faint,
    'titleBar.border': c.line,
  },
  tokenColors: [
    { scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: c.faint, fontStyle: 'italic' } },
    { scope: ['keyword', 'storage', 'storage.type', 'keyword.control'], settings: { foreground: c.violet } },
    { scope: ['string', 'string.quoted', 'markup.inline.raw'], settings: { foreground: c.green } },
    { scope: ['variable.other.placeholder'], settings: { foreground: c.blue } },
    { scope: ['constant.numeric', 'constant.language', 'constant.character', 'support.constant'], settings: { foreground: c.pink } },
    { scope: ['entity.name.function', 'support.function', 'meta.function-call'], settings: { foreground: c.orange } },
    { scope: ['entity.name.type', 'support.type', 'entity.name.class'], settings: { foreground: c.aqua } },
    { scope: ['variable', 'variable.parameter', 'meta.definition.variable'], settings: { foreground: c.yellow } },
    { scope: ['punctuation', 'keyword.operator', 'meta.brace'], settings: { foreground: '#9CABCA' } },
    { scope: ['entity.name.tag', 'entity.other.attribute-name', 'support.type.property-name'], settings: { foreground: c.blue } },
    { scope: ['markup.heading'], settings: { foreground: c.ink, fontStyle: 'bold' } },
    { scope: ['markup.inserted'], settings: { foreground: c.green } },
    { scope: ['markup.deleted', 'invalid'], settings: { foreground: c.red } },
  ],
};
