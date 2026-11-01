import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { sourceEnabled, enabledSources, withSourceEnabled, sourceLocatorEnabled, MUSIC_SOURCES } from '../src/utils/musicSources.ts';
import { initialMediaSource, createMainSectionPreference } from '../src/utils/appBootstrap.ts';
import { configuredExploreSources } from '../src/utils/exploreNavigation.ts';
const config={first_run:false,local_folders:['/fixture'],plex_token:'fixture-token',plex_url:'http://fixture.invalid/',sources:null};
test('legacy source preferences preserve all existing sources without eagerly serializing a migration',()=>{
 assert.deepEqual(enabledSources(config),{local:true,plex:true,navidrome:true});
 assert.equal(sourceEnabled(null,'local'),false);
 const before=JSON.stringify(config),next=withSourceEnabled(config,'plex',false);
 assert.equal(JSON.stringify(config),before);assert.equal(next.plex_token,config.plex_token);assert.equal(next.local_folders,config.local_folders);
 assert.deepEqual(next.sources,{local:true,plex:false,navidrome:true});
});
for(let mask=0;mask<8;mask++)test(`source configuration ${mask}: restore, Explore and typed media honor each independent switch`,()=>{
 const sources=Object.fromEntries(MUSIC_SOURCES.map((name,index)=>[name,!!(mask&(1<<index))]));const cfg={...config,sources};
 for(const name of MUSIC_SOURCES){assert.equal(sourceEnabled(cfg,name),sources[name]);assert.equal(sourceLocatorEnabled(cfg,{kind:name}),sources[name]);}
 for(const kind of ['radio','upnp'])assert.equal(sourceLocatorEnabled(cfg,{kind}),true);
 assert.deepEqual(configuredExploreSources(cfg),['local','plex'].filter(source=>sources[source]));
 assert.deepEqual(configuredExploreSources(cfg,true),MUSIC_SOURCES.filter(source=>sources[source]));
 const initial=sources.plex?'plex':sources.local?'local':sources.navidrome?'navidrome':'radio';assert.equal(initialMediaSource(cfg),initial);
 for(const saved of MUSIC_SOURCES){const writes=[],published=[];const preference=createMainSectionPreference(value=>published.push(value),()=>({getItem:()=>saved,setItem:(...args)=>writes.push(args)}));preference.restore(cfg);assert.equal(published.at(-1),sources[saved]?saved:initial);assert.deepEqual(writes,[]);}
});
test('disabled source navigation selects its body notice without opening Settings or loading a catalog',()=>{
 const source=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
 const actions={};const visit=node=>{if(ts.isJsxSelfClosingElement(node)&&node.tagName.getText(ast)==='SidebarNavItem'){
  const name=node.attributes.properties.find(p=>p.name?.getText(ast)==='diagnosticAction')?.initializer?.text;
  if(MUSIC_SOURCES.includes(name))actions[name]=node.attributes.properties.find(p=>p.name?.getText(ast)==='onClick')?.initializer?.expression;
 }ts.forEachChild(node,visit);};visit(ast);
 for(const name of MUSIC_SOURCES){const calls=[];const env={sourceEnabled,config:{sources:{local:false,plex:false,navidrome:false}},configureSource:()=>assert.fail('Settings opened automatically'),navigateLocalSubview:()=>calls.push('local'),resetAllNavigation(){},setPlexArea(){},setNavidromeArea(){},mainSectionPreference:{navigate:source=>calls.push(source)}};
  const code=ts.transpileModule('const handler='+actions[name].getText(ast),{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  new Function(...Object.keys(env),code+'; return handler;')(...Object.values(env))();assert.deepEqual(calls,[name]);
 }
});
