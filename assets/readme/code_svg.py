#!/usr/bin/env python3
import re, sys

# ---- themes ----
THEMES={
 'dark':{
  'bg':"#17130d",'title_bg':"#201a12",'hair':"#2c2519",'border':"#2a241c",
  'text':"#e8e0d2",'keyword':"#f2a541",'string':"#9ccf7a",'accessor':"#5fb9d6",
  'comment':"#6f6659",'builtin':"#e0a56a",'number':"#d98f6a",'punct':"#9a8f7d",
  'ws':"#e8e0d2",'prompt':"#f2a541",'cmdtext':"#e8e0d2",'output':"#b0a595",
  'label':"#6f6659",'dot1':"#e0603a",'dot2':"#f0a83a",'dot3':"#8a8072",
 },
 'light':{
  'bg':"#faf8f3",'title_bg':"#f0ebe1",'hair':"#e6e0d3",'border':"#e4ddce",
  'text':"#33302a",'keyword':"#bd5b12",'string':"#3f8020",'accessor':"#0e7490",
  'comment':"#8f8676",'builtin':"#a8571f",'number':"#b5540f",'punct':"#77716a",
  'ws':"#33302a",'prompt':"#bd5b12",'cmdtext':"#33302a",'output':"#6f685c",
  'label':"#a79e8c",'dot1':"#e0603a",'dot2':"#e0972a",'dot3':"#cfc8ba",
 },
}
F=15; LH=25; PADX=24; TITLE=42; TOP=16; BOT=16; CW=9.1
MONO="ui-monospace, 'SF Mono', 'SFMono-Regular', Menlo, Consolas, 'Liberation Mono', monospace"

COOK_KW={'recipe','chore','probe','ingredients','cook','seal','unseal','import',
         'tools','json','nondet','config','envs','env'}
LUA_KW={'local','function','end','if','then','else','elseif','return','for','in',
        'do','while','repeat','until','nil','true','false','and','or','not'}
LUA_BI={'fs','read','write','input','output','print'}

def esc(s): return s.replace('&','&amp;').replace('<','&lt;').replace('>','&gt;')

def split_string(s):
    out=[]; last=0
    for m in re.finditer(r'\$<[^>]*>', s):
        if m.start()>last: out.append((s[last:m.start()],'string'))
        out.append((m.group(0),'accessor')); last=m.end()
    if last<len(s): out.append((s[last:],'string'))
    return out

def tok_cook(line):
    spans=[]; i=0; n=len(line); seen=False
    while i<n:
        c=line[i]
        if c=='#': spans.append((line[i:],'comment')); break
        if c=='"':
            j=i+1
            while j<n and line[j]!='"':
                j+= 2 if line[j]=='\\' else 1
            j=min(j+1,n); spans+=split_string(line[i:j]); i=j; continue
        if c=='$' and i+1<n and line[i+1]=='<':
            j=line.find('>',i); j=j+1 if j!=-1 else n
            spans.append((line[i:j],'accessor')); i=j; continue
        if line[i:i+2]=='>{': spans.append(('>{','punct')); i+=2; continue
        if c in '{}': spans.append((c,'punct')); i+=1; continue
        m=re.match(r'[A-Za-z_][A-Za-z0-9_]*',line[i:])
        if m:
            w=m.group(0); cls='text'
            if not seen and w in COOK_KW: cls='keyword'
            elif w in LUA_KW: cls='keyword'
            elif w in LUA_BI: cls='builtin'
            spans.append((w,cls)); seen=True; i+=m.end(); continue
        m=re.match(r'\s+',line[i:])
        if m: spans.append((m.group(0),'ws')); i+=m.end(); continue
        m=re.match(r'\d+',line[i:])
        if m: spans.append((m.group(0),'number')); i+=m.end(); continue
        spans.append((c,'punct')); i+=1
    return spans

def tok_console(line):
    if line.startswith('$'):
        rest=line[1:]; ci=rest.find('#')
        # colour the invoked tool (first word) as keyword
        mt=re.match(r'(\s*)(\S+)',rest)
        spans=[('$','prompt')]
        head=rest if ci==-1 else rest[:ci]
        if mt and mt.end(2)<=len(head):
            spans.append((mt.group(1),'ws')); spans.append((mt.group(2),'keyword'))
            spans.append((head[mt.end(2):],'cmdtext'))
        else:
            spans.append((head,'cmdtext'))
        if ci!=-1: spans.append((rest[ci:],'comment'))
        return spans
    return [(line,'output')]

def render(code, lang='cook', label='Cookfile', theme='dark'):
    COL=THEMES[theme]; BG=COL['bg']; BORDER=COL['border']; HAIR=COL['hair']
    lines=code.split('\n')
    while lines and lines[-1]=='': lines.pop()
    tok = tok_cook if lang=='cook' else tok_console
    rows=[tok(l) if l else [] for l in lines]
    maxlen=max((len(l) for l in lines), default=10)
    W=int(maxlen*CW + 2*PADX + 0.5)
    W=max(W, 240)
    H=TITLE + TOP + len(lines)*LH + BOT
    out=[]
    kind = 'terminal session' if lang=='console' else 'cook code'
    out.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
               f'viewBox="0 0 {W} {H}" role="img" aria-labelledby="t">')
    out.append(f'<title id="t">{esc(label)} — {kind} snippet</title>')
    out.append(f'<rect width="{W}" height="{H}" rx="11" fill="{BG}" stroke="{BORDER}"/>')
    out.append(f'<path d="M0 {TITLE} H{W}" stroke="{HAIR}"/>')
    out.append(f'<circle cx="22" cy="21" r="5" fill="{COL["dot1"]}"/>'
               f'<circle cx="40" cy="21" r="5" fill="{COL["dot2"]}"/>'
               f'<circle cx="58" cy="21" r="5" fill="{COL["dot3"]}"/>')
    out.append(f'<text x="{W-PADX}" y="26" text-anchor="end" font-family="{MONO}" '
               f'font-size="13" fill="{COL["label"]}">{esc(label)}</text>')
    y=TITLE+TOP+13
    for spans in rows:
        if spans:
            parts=[]
            for txt,cls in spans:
                if txt=='': continue
                fill=COL.get(cls,COL['text'])
                weight=' font-weight="600"' if cls=='keyword' else ''
                parts.append(f'<tspan fill="{fill}"{weight}>{esc(txt)}</tspan>')
            out.append(f'<text x="{PADX}" y="{y}" xml:space="preserve" font-family="{MONO}" '
                       f'font-size="{F}">{"".join(parts)}</text>')
        y+=LH
    out.append('</svg>')
    return '\n'.join(out)

# ---- README.source pipeline (driven by the root Cookfile) ----
#
# README.source is README.md with each <picture> code snippet written as an
# annotated fence:
#
#   <!-- alt: accessible description of the snippet -->
#   ```cook sprite
#   recipe sprite-sheet
#   ...
#   ```
#
# `snippets` emits the fences as JSON (the readme:snippets probe's value —
# one record per fence, so cook fans out one render unit per snippet and a
# code edit re-keys only its own snippet). `render` regenerates one snippet
# SVG. `compile` writes README.md with every fence replaced by the
# light/dark <picture> block.

FENCE=re.compile(
    r'<!-- alt: (?P<alt>.*?) -->\n'
    r'```(?P<lang>cook|console) (?P<name>[a-z][a-z0-9-]*)\n'
    r'(?P<code>.*?)\n```',
    re.S)

def parse_source(text):
    out=[]
    for m in FENCE.finditer(text):
        lang=m.group('lang')
        out.append({
            'name': m.group('name'),
            'lang': lang,
            'label': 'terminal' if lang=='console' else 'Cookfile',
            'alt': m.group('alt'),
            'code': m.group('code'),
        })
    return out

def compile_readme(text):
    def repl(m):
        name=m.group('name')
        return (
            '<picture>\n'
            f'  <source media="(prefers-color-scheme: dark)" srcset="assets/readme/snippet-{name}-dark.svg">\n'
            f'  <img src="assets/readme/snippet-{name}.svg" alt="{m.group("alt")}">\n'
            '</picture>'
        )
    return FENCE.sub(repl, text)

if __name__=='__main__':
    import json
    mode=sys.argv[1]
    text=open(sys.argv[2]).read()
    if mode=='snippets':
        print(json.dumps(parse_source(text)))
    elif mode=='render':
        name, theme = sys.argv[3], sys.argv[4]
        recs={r['name']: r for r in parse_source(text)}
        r=recs[name]
        sys.stdout.write(render(r['code'], r['lang'], r['label'], theme))
    elif mode=='compile':
        sys.stdout.write(compile_readme(text))
    else:
        sys.exit(f"unknown mode: {mode} (want snippets|render|compile)")
