const $ = id => document.getElementById(id);
const NS = 'http://www.w3.org/2000/svg';
const C = {pink:'#ed286c',mint:'#9dcea0',blue:'#8dbbd4',lilac:'#b59ae2',dim:'#40454f'};
const duration = 18, exportMode = new URLSearchParams(location.search).get('export') === '1';
document.body.classList.toggle('export', exportMode);
const clamp = n => Math.max(0, Math.min(1,n));
const ease = n => { n=clamp(n); return n*n*(3-2*n); };
function el(tag, attrs, parent, content) {
  const e=document.createElementNS(NS,tag);
  for(const [k,v] of Object.entries(attrs)) e.setAttribute(k,v);
  if(content !== undefined) e.textContent=content;
  parent.append(e); return e;
}
function text(parent,x,y,content,cls='detail',attrs={}) { return el('text',{x,y,class:cls,...attrs},parent,content); }
function node(parent,x,y,title,detail,color=C.blue,w=190) {
  const g=el('g',{},parent);
  const box=el('rect',{x:x-w/2,y:y-38,width:w,height:76,rx:9,class:'node'},g);
  const dot=el('circle',{cx:x-w/2+15,cy:y-20,r:3,fill:color},g);
  text(g,x,y-1,title,'label',{'text-anchor':'middle'});
  const sub=text(g,x,y+23,detail,'detail',{'text-anchor':'middle'});
  return {g,box,dot,sub,color};
}
const paths=[];
function stream(parent,d,color=C.blue,birth=0) {
  const g=el('g',{},parent);
  // Fixed strands, sampled once: seeking never depends on a previous frame.
  for(let i=-3;i<=3;i++) el('path',{d,fill:'none',stroke:color,'stroke-width':.65,opacity:i===0?.3:.09,transform:`translate(0 ${i*2})`},g);
  const guide=el('path',{d,fill:'none',stroke:color,'stroke-width':1.1,opacity:.32},g);
  const length=guide.getTotalLength();
  const points=Array.from({length:241},(_,i)=>{const p=guide.getPointAtLength(length*i/240);return [p.x,p.y]});
  const r={g,guide,points,birth,length,color};paths.push(r);return r;
}
const flights=[];
function flight(path,start,end,color=path.color,lane=0) {
  const g=el('g',{},path.g), dots=[];
  for(let i=7;i>=0;i--) dots.push(el('circle',{r:i===0?3.1:Math.max(.8,2.3-i*.2),fill:color,opacity:1-i/8},g));
  const bloom=el('circle',{r:6,fill:color,filter:'url(#glow)',opacity:.55},g);
  flights.push({path,start,end,g,dots,bloom,lane});
}
function burst(path,start,end,count=5,color=path.color) {
  for(let i=0;i<count;i++) flight(path,start+i*.085,end+i*.085,color,(i%3-1)*3);
}
const chapterNames=['01  LOWER & REGISTER','02  BUILD THE DAG','03  EXECUTE WHAT IS READY'];
const chapterNodes=chapterNames.map((s,i)=>text($('chapters'),64+i*440,173,s,'mono eyebrow'));
const front=$('front'),dag=$('dag'),engine=$('engine');
// Act 1: emission and capture are distinct phases.
const fx=[155,415,685,980,1255];
const frontEdges=fx.slice(0,-1).map((x,i)=>stream(front,`M${x+96} 390 C${x+150} 355 ${fx[i+1]-150} 425 ${fx[i+1]-96} 390`,i<2?C.blue:C.lilac));
frontEdges.forEach((p,i)=>burst(p,.45+i*.85,1.03+i*.85,6));
const frontNodes=[node(front,fx[0],390,'Cookfile','source declarations'),node(front,fx[1],390,'Parse','cook-lang'),node(front,fx[2],390,'luagen','emit Lua',C.pink),node(front,fx[3],390,'Register','capture units',C.lilac),node(front,fx[4],390,'Work units','+ declared edges',C.mint)];
text(front,685,293,'AST → REGISTER-PHASE LUA','mono eyebrow',{'text-anchor':'middle',fill:C.pink});
text(front,829,353,'Lua','detail',{'text-anchor':'middle',fill:C.lilac});
el('path',{d:'M589 452V483H1075V452',fill:'none',stroke:'#40454f'},front);
text(front,832,515,'Generated code declares the work.','',{'text-anchor':'middle','font-size':22});
text(front,832,546,'cook.add_unit(...)  ·  cook.exec(...)','detail',{'text-anchor':'middle',fill:C.lilac});
text(front,64,663,'01 / EMIT','mono eyebrow',{fill:C.pink});text(front,64,700,'Lower the parsed Cookfile into Lua.','',{'font-size':23});
text(front,735,663,'02 / CAPTURE','mono eyebrow',{fill:C.lilac});text(front,735,700,'Run registration; collect units and dependencies.','',{'font-size':23});
text(front,735,733,'cook.exec captures a unit here; workers run it later.','detail');
// A six-unit example. One cached branch, two concurrent worker jobs.
const units=[
 {name:'compile a',detail:'a.c → a.o',x:240,y:335,deps:[],start:9.15,gate:9.50,work:9.92,done:11.25,worker:0},
 {name:'compile b',detail:'b.c → b.o',x:240,y:470,deps:[],start:9.25,gate:9.60,work:10.02,done:11.55,worker:1},
 {name:'compile c',detail:'c.c → c.o',x:240,y:605,deps:[],start:9.35,gate:9.70,work:10.12,done:10.50,hit:true},
 {name:'link',detail:'objects → app',x:590,y:470,deps:[0,1,2],start:11.80,gate:12.10,work:12.40,done:13.30,worker:0},
 {name:'test',detail:'verify app',x:900,y:470,deps:[3],start:13.55,gate:13.85,work:14.15,done:14.90,worker:1},
 {name:'package',detail:'release artifact',x:1210,y:470,deps:[4],start:15.15,gate:15.45,work:15.75,done:16.60,worker:0},
];
const dagEdges=[];
units.forEach((u,i)=>u.deps.forEach(dep=>{
 const a=units[dep],d=`M${a.x+105} ${a.y} C${a.x+190} ${a.y} ${u.x-195} ${u.y} ${u.x-105} ${u.y}`;
 const p=stream(dag,d,C.mint,5.95+i*.24);dagEdges.push(p);
 burst(p,p.birth,p.birth+.5,3,C.lilac);
 flight(p,a.done,a.done+.25,C.mint);
}));
units.forEach((u,i)=>u.view=node(dag,u.x,u.y,u.name,u.detail,C.lilac,210));
const dagTitle=text(dag,64,249,'ONE WORK-UNIT DAG','mono eyebrow',{fill:C.lilac});
const dagNote=text(dag,1376,249,'declared dependencies · parallel branches · ordered steps','detail',{'text-anchor':'end'});
const buildNote=text(dag,720,725,'Edges encode declared ordering. Independent nodes can run together.','',{'text-anchor':'middle','font-size':24});
// Act 3: the same cache decision is made for each ready unit.
text(engine,64,585,'PER READY UNIT','mono eyebrow',{fill:C.blue});
const readyPath=stream(engine,'M250 672 C300 660 332 660 387 672',C.blue);
const missPaths=[stream(engine,'M510 652 C570 612 611 620 666 620',C.pink),stream(engine,'M510 672 C570 692 611 709 666 709',C.pink)];
const hitPath=stream(engine,'M450 712 C500 801 900 801 1070 711',C.mint);
const resultPaths=[stream(engine,'M866 620 C945 620 967 672 1050 672',C.blue),stream(engine,'M866 709 C945 709 967 687 1050 687',C.blue)];
const readyNode=node(engine,155,672,'Ready','remaining deps = 0',C.blue);
const gateGroup=el('g',{},engine);
const gate=el('path',{d:'M450 625L513 672L450 719L387 672Z',fill:'#191c23',stroke:C.dim,'stroke-width':1.6},gateGroup);
text(gateGroup,450,669,'CACHE','mono',{'text-anchor':'middle','font-size':16});text(gateGroup,450,690,'hit?','detail',{'text-anchor':'middle'});
text(engine,553,611,'MISS','detail',{fill:C.pink});text(engine,550,782,'HIT · REUSE / RESTORE','detail',{fill:C.mint});
const workerNodes=[node(engine,766,620,'Worker 01','Lua VM / shell',C.blue,200),node(engine,766,709,'Worker 02','Lua VM / shell',C.blue,200)];
const workerBars=workerNodes.map((n,i)=>el('rect',{x:683,y:(i===0?620:709)+30,height:2,width:0,fill:C.blue},n.g));
const completeNode=node(engine,1160,677,'Complete','release dependents',C.mint,220);
units.forEach(u=>{
 flight(readyPath,u.start,u.gate,C.lilac);
 if(u.hit) flight(hitPath,u.gate,u.done,C.mint);
 else {flight(missPaths[u.worker],u.gate,u.work,C.pink);flight(resultPaths[u.worker],u.done-.30,u.done,C.blue);}
});
// Mobile is recomposed as a vertical phase story rather than a shrunken desktop.
const mobileLabels=[['Cookfile → AST','parse declarations'],['luagen → Lua','emit registration program'],['Register → DAG','capture units + wire edges'],['Cache → workers','hits reuse · misses execute'],['Complete → release','unlock dependent units']];
const mobileNodes=mobileLabels.map(([a,b],i)=>{
 if(i<4) stream($('mobile'),`M210 ${206+i*94} C210 ${225+i*94} 210 ${230+i*94} 210 ${243+i*94}`,C.blue);
 return node($('mobile'),210,174+i*94,a,b,[C.blue,C.pink,C.lilac,C.blue,C.mint][i],330);
});
function accent(n,active,done=false){n.box.style.stroke=done?C.mint:active?n.color:C.dim;n.box.style.fill=active?'#252630':'#191c23';n.dot.setAttribute('fill',done?C.mint:n.color);}
let current=0;
function render(t) {
 current=Math.max(0,Math.min(duration,Number(t)||0));t=current;
 const stage=t<5?0:t<9?1:2, shift=ease((t-8.15)/.75)*80;
 front.setAttribute('opacity',1-ease((t-4.7)/.4));front.style.visibility=t>=5.1?'hidden':'visible';
 dag.setAttribute('opacity',ease((t-5)/.4));dag.style.visibility=t<5?'hidden':'visible';dag.setAttribute('transform',`translate(0 ${-shift})`);
 engine.setAttribute('opacity',ease((t-8.65)/.35));engine.style.visibility=t<8.65?'hidden':'visible';
 dagTitle.setAttribute('opacity',1-shift/80);dagNote.setAttribute('opacity',1-shift/80);buildNote.setAttribute('opacity',1-shift/80);
 chapterNodes.forEach((n,i)=>n.setAttribute('fill',i===stage?[C.pink,C.lilac,C.mint][i]:'#737a86'));
 frontNodes.forEach((n,i)=>accent(n,t>=i*.85&&t<(i+1)*.85,t>=(i+1)*.85));
 paths.forEach(p=>{const reveal=ease((t-p.birth)/.6);p.g.setAttribute('opacity',reveal);p.guide.setAttribute('stroke-dasharray',`${p.length*reveal} ${p.length}`);});
 let complete=0,hits=0,running=0;
 units.forEach((u,i)=>{
  const done=t>=u.done,active=t>=u.start&&!done,waiting=u.deps.filter(d=>t<units[d].done+.25).length;
  if(done){complete++;if(u.hit)hits++;}if(!u.hit&&t>=u.work&&t<u.done-.3)running++;
  u.view.g.setAttribute('opacity',ease((t-5.2-i*.20)/.45));accent(u.view,active,done);
  u.view.sub.textContent=stage<2?u.detail:done?(u.hit?'cached · complete':'complete'):active?(t<u.gate?'ready':u.hit?'cache hit':t<u.work?'cache miss':'running'):waiting?`${waiting} ${waiting===1?'dependency':'dependencies'} remaining`:'ready';
 });
 workerNodes.forEach((n,i)=>{const u=units.find(u=>!u.hit&&u.worker===i&&t>=u.work&&t<u.done-.3);accent(n,!!u);n.sub.textContent=u?u.name:'Lua VM / shell';workerBars[i].setAttribute('width',u?166*clamp((t-u.work)/(u.done-.3-u.work)):0);});
 const checking=units.some(u=>t>=u.gate-.1&&t<u.gate+.22);gate.setAttribute('stroke',checking?C.pink:C.dim);
 accent(readyNode,units.some(u=>t>=u.start&&t<u.gate));accent(completeNode,units.some(u=>t>=u.done-.12&&t<u.done+.3),complete===6);
 completeNode.sub.textContent=complete===6?'6 / 6 units complete':'release dependents';
 flights.forEach(f=>{
  const active=t>=f.start&&t<=f.end;f.g.style.display=active?'':'none';if(!active)return;
  const progress=(t-f.start)/(f.end-f.start);
  f.dots.forEach((dot,i)=>{
   const p=clamp(progress-(7-i)*.024),v=f.path.points[Math.round(p*240)];
   dot.setAttribute('cx',v[0]);dot.setAttribute('cy',v[1]+f.lane);dot.setAttribute('opacity',p===0&&i<7?0:(i+1)/8);
  });
  const v=f.path.points[Math.round(progress*240)];f.bloom.setAttribute('cx',v[0]);f.bloom.setAttribute('cy',v[1]+f.lane);
 });
 const caption=t<1.8?'Parse declarations. Lower the AST.':t<3.3?'Luagen emits Lua that declares work.':t<5?'Registration captures units and their dependencies.':t<7.8?'Build one DAG from the declared ordering.':t<9?'Three independent compile units are ready together.':t<10.6?'Cache hits reuse results; misses flow to workers.':t<11.8?'Independent misses execute in parallel. Link waits for all three.':t<13.55?'The final compile completes. Link is now ready.':t<15.15?'Link releases test. Test gates the package.':t<16.6?'All prerequisites satisfied. Produce the release artifact.':'Build complete. Six units resolved; only five needed execution.';
 $('caption').textContent=caption;
 $('counts').textContent=stage<2?'6 EXAMPLE UNITS / SIMULATED TIMING':`${complete} / 6 COMPLETE     ${hits} CACHE HIT     ${running} RUNNING`;
 $('progress').setAttribute('width',1440*t/duration);
 const m=t<1.8?0:t<3.3?1:t<9?2:t<16.6?3:4;
 mobileNodes.forEach((n,i)=>accent(n,i===m,i<m));
 $('mobile-caption').textContent=['Parse the source.','Emit register-phase Lua.','Build the dependency graph.','Resolve ready units.','Release dependent work.'][m];
 $('mobile-counts').textContent=`${complete}/6 complete · ${hits} cached · ${running} running`;
 $('seek').value=t;$('time').value=`${t.toFixed(1)} / 18s`;
 return {stage,complete,hits,running};
}
await document.fonts.ready;
window.motionGraphic={duration,render};
const media=matchMedia('(max-width:650px)'), reduced=matchMedia('(prefers-reduced-motion:reduce)');
function resize(){ $('graphic').setAttribute('viewBox',media.matches&&!exportMode?'0 0 420 740':'0 0 1440 900');render(current); }
media.addEventListener('change',resize);resize();
let playing=!exportMode&&!reduced.matches, raf=0,last=0,visible=true;
function sync(){ $('play').textContent=playing?'Pause':current>=duration?'Replay':'Play';cancelAnimationFrame(raf);last=0;if(playing&&visible&&!document.hidden&&!exportMode)raf=requestAnimationFrame(tick); }
function tick(now){if(last)render(Math.min(duration,current+(now-last)/1000));last=now;if(current>=duration){playing=false;sync();}else raf=requestAnimationFrame(tick);}
$('play').addEventListener('click',()=>{if(current>=duration)render(0);playing=!playing;sync();});
$('seek').addEventListener('input',e=>{playing=false;render(+e.target.value);sync();});
document.addEventListener('visibilitychange',sync);
const observer=new IntersectionObserver(([entry])=>{visible=entry.isIntersecting;sync();});observer.observe($('graphic'));
reduced.addEventListener('change',()=>{if(reduced.matches){playing=false;sync();}});
window.addEventListener('pagehide',()=>{cancelAnimationFrame(raf);observer.disconnect();},{once:true});
render(reduced.matches&&!exportMode?11:0);sync();
