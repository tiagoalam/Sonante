import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import {createMainSectionPreference, disabledAutoEqFallback, initialMediaSource} from '../src/utils/appBootstrap.ts';

const source=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');
const ast=ts.createSourceFile('App.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
const walk=fn=>{const visit=n=>{fn(n);ts.forEachChild(n,visit)};visit(ast)};
function expression(name){let value;walk(n=>{if(ts.isVariableDeclaration(n)&&n.name.getText(ast)===name)value=n.initializer.getText(ast)});assert.ok(value,name);return value;}
function navItem(label){let item;walk(n=>{
 if(ts.isJsxSelfClosingElement(n)&&n.tagName.getText(ast)==='SidebarNavItem'
  &&n.attributes.properties.some(p=>p.name?.getText(ast)==='label'&&p.initializer?.getText(ast)===`{t("${label}")}`))item=n;
 });return item;
}

test('sidebar utilities put Equalizer before Preferences without changing route availability',()=>{
 const start=source.indexOf('<nav aria-label={t("sidebar.utilitiesGroup")}');
 const end=source.indexOf('</nav>',start);
 assert.ok(start>=0&&end>start);
 const utilities=source.slice(start,end);
 assert.ok(utilities.indexOf('equalizer.title')<utilities.indexOf('<KeyboardShortcutsHelp />'));
 assert.equal(utilities.includes('autoeq.title'),false,'AutoEQ is an internal Equalizer tab, not a sidebar entry');
 assert.ok(utilities.indexOf('<KeyboardShortcutsHelp />')<utilities.indexOf('sidebar.preferences'));
 const item=navItem('equalizer.title');assert.ok(item);
 assert.match(item.attributes.properties.map(p=>p.getText(ast)).join(' '),/sourceEnabled=\{equalizerActive\}/);
});

test('Equalizer remains an independent sidebar route and legacy stored route restores it',()=>{
 const item=navItem('equalizer.title');assert.ok(item);
 assert.equal(navItem('settings.pages.audio'),undefined);
 const saved=[],published=[];
 const preference=createMainSectionPreference(section=>published.push(section),()=>({getItem:()=>"equalizer",setItem:(_key,value)=>saved.push(value)}),undefined,initialMediaSource);
 preference.restore({first_run:false,plex_token:'fixture',usage_mode:'full'});
 assert.equal(published.at(-1),'equalizer');assert.deepEqual(saved,[]);
 preference.navigate('equalizer');assert.equal(published.at(-1),'equalizer');assert.deepEqual(saved,['equalizer']);
});

test('Equalizer shows one manual page and legacy AutoEQ navigation falls back to it',()=>{
 assert.equal(navItem('autoeq.title'),undefined);
 assert.doesNotMatch(source,/role="tablist"|autoeq-tab|AutoEqView|OpraCatalogBrowser|data-autoeq-page/);
 assert.doesNotMatch(readFileSync(new URL('../src/components/deferredViews.tsx',import.meta.url),'utf8'),/AutoEqView|OpraCatalogBrowser/);
 assert.match(source,/const showManualEqualizer = mediaSource === "equalizer" \|\| mediaSource === "autoeq"/);
 const saved=[],published=[];
 const preference=createMainSectionPreference(section=>published.push(section),()=>({getItem:()=>"autoeq",setItem:(_key,value)=>saved.push(value)}),undefined,initialMediaSource);
 preference.restore({first_run:false,plex_token:'fixture',usage_mode:'full'});
 assert.equal(published.at(-1),'autoeq');assert.deepEqual(saved,[]);
 preference.navigate('autoeq');assert.equal(published.at(-1),'autoeq');assert.deepEqual(saved,['autoeq']);
 assert.match(source,/mediaSource === "equalizer" \|\| mediaSource === "autoeq"/);
 assert.match(source,/disabledAutoEqFallback\(mediaSource, experimental\)/);
 assert.match(source,/disabledAutoEqFallback\(playerReturnSection, experimental\)/);
 assert.match(source,/!showManualEqualizer \? "hidden"/);
});

test('Beta settings remain available but never expose or mount AutoEQ',()=>{
 assert.equal(disabledAutoEqFallback('autoeq',false),'equalizer');
 assert.equal(disabledAutoEqFallback('autoeq',true),null);
 assert.equal(disabledAutoEqFallback('equalizer',false),null);
 assert.match(source,/const experimental = experimentalFeaturesEnabled\(config\) && !config\?\.first_run/);
 assert.doesNotMatch(source,/AutoEqView|OpraCatalogBrowser|opra_catalog/);
 const settings=readFileSync(new URL('../src/components/SettingsModal.tsx',import.meta.url),'utf8');
 assert.match(settings,/ExperimentalFeaturesSetting enabled=\{config\.experimental_features === true\}/);
 assert.doesNotMatch(source,/setConfig\([^)]*autoeq|experimental_features[^;]*autoeq/);
});

test('Equalizer navigation remembers the source page and stays clickable while EQ is off',()=>{
 const item=navItem('equalizer.title');assert.ok(item);
 const attrs=Object.fromEntries(item.attributes.properties.map(p=>[p.name.getText(ast),p.initializer?.getText(ast)]));
 assert.match(attrs.sourceEnabled,/equalizerActive/);
 const click=item.attributes.properties.find(p=>p.name?.getText(ast)==='onClick').initializer.expression.getText(ast);
 const code=ts.transpileModule(`const open=${click}; open();`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
 const events=[],ref={current:'local'};
 new Function('mediaSource','eqReturnSectionRef','resetAllNavigation','mainSectionPreference',code)('favorites',ref,()=>events.push('reset'),{navigate:section=>events.push(section)});
 assert.equal(ref.current,'favorites');assert.deepEqual(events,['reset','equalizer']);
 const html=source;
 assert.match(html,/sourceEnabled=\{equalizerActive\}/);assert.match(html,/data-equalizer-disabled/);
});

test('inactive Equalizer page links directly to Audio settings; editor renders only from runtime state',()=>{
 assert.match(source,/\{equalizerActive\}[\s\S]*?<EqualizerView/);
 assert.match(source,/configuredSignalRoute\(config\)\?\.mode === "direct"[\s\S]*?equalizer\.disabledExclusive/);
 const button=Array.from(source.matchAll(/setSettingsInitialPage\("audio"\); setShowSettings\(true\)/g));
 assert.ok(button.length>=1);
 assert.doesNotMatch(source,/label=\{t\("settings\.pages\.audio"\)\}/);
 assert.match(source,/active=\{mediaSource === "equalizer" \|\| mediaSource === "autoeq"\} sourceEnabled=\{equalizerActive\}/);
 assert.match(source,/mediaSource === "equalizer" \|\| mediaSource === "autoeq" \? "hidden"/);
 assert.equal(expression('equalizerActive'),'fullMode && equalizerIsActive(playbackStatus.shared_dsp, playbackStatus.audio_route)');
});

test('Equalizer goes directly to Local even when Local is the hidden return section',()=>{
 const handler=expression('navigateLocalSubview');
 const code=ts.transpileModule(`const navigate=${handler}; navigate("browser");`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
 for(const equalizerActive of [true,false]){
  const events=[];
  new Function('playerTarget','mediaSource','centralSection','setLocalSubview','changePlayerLocalSubview','resetAllNavigation','mainSectionPreference',code)(
   null,'equalizer','local',view=>events.push(['subview',view]),view=>events.push(['player-subview',view]),()=>events.push(['reset']),{navigate:section=>events.push(['section',section])});
  assert.deepEqual(events,[['subview','browser'],['reset'],['section','local']],
   `Local navigation must leave the Equalizer page whether the editor is ${equalizerActive?'active':'inactive'}`);
 }
 assert.match(source,/if \(!playerTarget && mediaSource === "local"\)/);
 assert.doesNotMatch(handler,/audioService|configService|saveConfig/);
});

test('Equalizer can return to the previous section after opening from navigation',()=>{
 const item=navItem('equalizer.title');assert.ok(item);
 const click=item.attributes.properties.find(p=>p.name?.getText(ast)==='onClick').initializer.expression.getText(ast);
 const code=ts.transpileModule(`const open=${click}; open();`,{compilerOptions:{target:ts.ScriptTarget.ES2020}}).outputText;
 const events=[],ref={current:'radio'};
 new Function('mediaSource','eqReturnSectionRef','resetAllNavigation','mainSectionPreference',code)('local',ref,()=>events.push('reset'),{navigate:section=>events.push(section)});
 assert.equal(ref.current,'local');assert.deepEqual(events,['reset','equalizer']);
 assert.match(source,/onReturn=\{\(\) => mainSectionPreference\.navigate\(eqReturnSectionRef\.current\)\}/);
 const equalizer=readFileSync(new URL('../src/components/EqualizerView.tsx',import.meta.url),'utf8');
 assert.match(equalizer,/onReturn\&&<button[^>]+className="eq-return/);
 assert.match(equalizer,/t\('equalizer\.return'\)/);
});

test('legacy AutoEQ route only exposes the existing manual page and does not load catalog UI',()=>{
 assert.match(source,/fullMode && \(mediaSource === "equalizer" \|\| mediaSource === "autoeq"\)/);
 assert.match(source,/data-equalizer-page/);
 assert.doesNotMatch(source,/data-autoeq-page|AutoEqView|OpraCatalogBrowser/);
 assert.match(source,/active=\{mediaSource === "equalizer" \|\| mediaSource === "autoeq"\} sourceEnabled=\{equalizerActive\}/);
 const equalizer=readFileSync(new URL('../src/components/EqualizerView.tsx',import.meta.url),'utf8');
 assert.doesNotMatch(equalizer,/OpraCatalogBrowser|apply_equalizer/);
});
