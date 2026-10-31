import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import {createMainSectionPreference} from '../src/utils/appBootstrap.ts';
import {exploreSourceFor} from '../src/utils/exploreNavigation.ts';
const root=process.env.SONANTE_NAV_BASE??new URL('../',import.meta.url).pathname;
const source=readFileSync(root+'/src/App.tsx','utf8');
const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
const walk=(fn)=>{const visit=n=>{fn(n);ts.forEachChild(n,visit)};visit(ast)};
function initializer(name){let value;walk(n=>{if(ts.isVariableDeclaration(n)&&n.name.getText(ast)===name)value=n.initializer.getText(ast)});assert.ok(value,name);return value}
function evaluate(code,env){const js=ts.transpileModule(`const extracted=(${code});`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;return new Function(...Object.keys(env),js+'return extracted;')(...Object.values(env))}
function sidebar(action){let code;walk(n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='SidebarNavItem'&&n.attributes.properties.some(p=>p.name?.getText(ast)==='diagnosticAction'&&p.initializer?.getText(ast)===JSON.stringify(action)))code=n.attributes.properties.find(p=>p.name?.getText(ast)==='onClick')?.initializer?.expression.getText(ast)});assert.ok(code,action);return code}
function setup(origin,target=null){
 const ctx={section:origin,subview:'artists',target,reset:0,transport:0};const saved=[];
 const pref=createMainSectionPreference(s=>ctx.section=s,()=>({getItem:()=>origin,setItem:(k,v)=>saved.push(v)}));pref.restore({first_run:false,plex_token:'fixture'});
 const returnRef={current:origin};
 const section=()=>evaluate(initializer('centralSection'),{mediaSource:ctx.section,playerReturnSection:ctx.target?origin:null,eqReturnSectionRef:returnRef});
 const env={mediaSource:'equalizer',playerTarget:target,centralSection:origin,eqReturnSectionRef:returnRef,eqReturnViewRef:{current:true},setLocalSubview:v=>ctx.subview=v,changePlayerLocalSubview:v=>ctx.playerSubview=v,resetAllNavigation:()=>{ctx.reset++;ctx.target=null},mainSectionPreference:pref};
 env.navigateExplore=evaluate(initializer('navigateExplore'),{...env,exploreSource:'local',exploreSources:['local','plex'],exploreSourceFor,setExploreSource:v=>ctx.exploreSource=v,setPlexArea:v=>ctx.plexArea=v});
 ctx.section='equalizer';return {ctx,env,section,saved};
}
test('actual Local handler exits EQ from Local/Explore/Plex/Playlists; Local return retains context',()=>{
 for(const origin of ['local','explore','plex','playlists']){
  const h=setup(origin);evaluate(initializer('navigateLocalSubview'),h.env)('browser');assert.equal(h.ctx.section,'local',origin);assert.equal(h.ctx.subview,'browser');assert.equal(h.ctx.reset,origin==='local'?0:1);assert.equal(h.ctx.transport,0);
 }
});
test('actual Local handler restores the retained Local player target instead of leaving EQ visible',()=>{
 for(const origin of ['local','explore','plex','playlists']){
  const target={kind:'local',albumId:'fixture'};const h=setup(origin,target);evaluate(initializer('navigateLocalSubview'),h.env)('browser');assert.equal(h.ctx.section,origin);assert.equal(h.ctx.target,target);assert.equal(h.ctx.playerSubview,'browser');assert.equal(h.ctx.reset,0);
 }
});
test('actual Explore sidebar callback leaves EQ from every origin and clears only navigation state',()=>{
 for(const origin of ['local','explore','plex','playlists'])for(const target of [null,{kind:'local'}]){
  const h=setup(origin,target);evaluate(sidebar('explore'),h.env)();assert.equal(h.ctx.section,'explore');assert.equal(h.ctx.reset,1);assert.equal(h.ctx.transport,0);assert.equal(h.section(),'explore');
 }
});
test('repeated EQ→Local→EQ→Explore clicks keep publishing the intended destination',()=>{
 let enter;walk(n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='SidebarNavItem'&&n.attributes.properties.some(p=>p.name?.getText(ast)==='label'&&p.initializer?.getText(ast)==='{t("equalizer.title")}'))enter=n.attributes.properties.find(p=>p.name?.getText(ast)==='onClick').initializer.expression.getText(ast)});assert.ok(enter);
 for(const origin of ['local','explore','plex','playlists']){
  const h=setup(origin);for(let i=0;i<5;i++){
   h.env.mediaSource=h.ctx.section;h.env.centralSection=h.section();evaluate(initializer('navigateLocalSubview'),h.env)('browser');assert.equal(h.ctx.section,'local');
   h.env.mediaSource=h.ctx.section;evaluate(enter,h.env)();assert.equal(h.ctx.section,'equalizer');assert.equal(h.env.eqReturnSectionRef.current,'local');
   h.env.mediaSource=h.ctx.section;evaluate(sidebar('explore'),h.env)();assert.equal(h.ctx.section,'explore');
   h.env.mediaSource=h.ctx.section;evaluate(enter,h.env)();assert.equal(h.ctx.section,'equalizer');assert.equal(h.env.eqReturnSectionRef.current,'explore');
  }
 }
});
test('retained Explore is inactive under EQ and initial direct EQ does not mount a previous page',()=>{
 let active,wrapper;walk(n=>{if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='ExploreView')active=n.attributes.properties.find(p=>p.name?.getText(ast)==='active').initializer.expression.getText(ast);if(ts.isJsxElement(n)&&n.openingElement.attributes.properties.some(p=>p.name?.getText(ast)==='data-now-playing-previous-page'))wrapper=n.openingElement});
 assert.ok(active&&wrapper);assert.equal(evaluate(active,{mediaSource:'equalizer',exploreDetail:null,playerTarget:null}),false);assert.equal(evaluate(active,{mediaSource:'explore',exploreDetail:null,playerTarget:null}),true);
 const visible=evaluate(initializer('exploreLive').replace(/^useRef\(/,'').replace(/\)$/,''),{exploreContext:{}});assert.ok(visible);
 const classExpr=wrapper.attributes.properties.find(p=>p.name?.getText(ast)==='className').initializer.expression.getText(ast),inertExpr=wrapper.attributes.properties.find(p=>p.name?.getText(ast)==='inert').initializer.expression.getText(ast);
 for(const mediaSource of ['local','explore','plex','playlists']){assert.equal(evaluate(classExpr,{mediaSource,nowPlayingEmbedded:false}),'contents');assert.equal(evaluate(inertExpr,{mediaSource,nowPlayingEmbedded:false}),undefined)}
 assert.match(source,/\(mediaSource !== "equalizer" \|\| eqReturnViewRef.current\) &&/);
});
