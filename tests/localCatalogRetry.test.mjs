import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import * as jsx from 'react/jsx-runtime';
import * as icons from 'lucide-react';
import i18next from 'i18next';
import {renderToStaticMarkup} from 'react-dom/server';
import * as catalogUtils from '../src/utils/localAlbumCatalog.ts';
const {LocalAlbumCatalog,canRetryLocalCatalog}=catalogUtils;
const read=name=>readFileSync(new URL('../src/'+name,import.meta.url),'utf8');
const nodes=(tree,p)=>Array.isArray(tree)?tree.flatMap(n=>nodes(n,p)):!tree||typeof tree!=='object'?[]:[...(p(tree)?[tree]:[]),...nodes(tree.props?.children,p)];
const tick=()=>new Promise(resolve=>setImmediate(resolve));
async function view(language){
 const i18n=i18next.createInstance();await i18n.init({lng:language,resources:{[language]:{translation:JSON.parse(read('locales/'+language+'.json'))}}});
 const deps={'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:i18n.t.bind(i18n)})},'../utils/localAlbumCatalog':catalogUtils},exports={};
 new Function('require','exports',ts.transpileModule(read('components/LocalCatalogReadError.tsx'),{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText)(name=>{assert.ok(name in deps,name);return deps[name]},exports);
 return{Notice:exports.LocalCatalogReadError,t:i18n.t.bind(i18n)};
}
const state={albums:[],loading:false,loaded:false,error:'SOURCE_AVAILABILITY_CHANGED',mpdBusy:false,saveBlocked:false};
for(const language of ['en','pt-BR'])test(`Local read failures offer a retry, never an empty collection or a recovery action (${language})`,async()=>{
 const{Notice,t}=await view(language);let clicked=0;
 const tree=Notice({catalog:state,available:true,updating:false,onRetry:()=>clicked++});const html=renderToStaticMarkup(tree);
 assert.match(html,/role="alert"/);assert.ok(html.includes(t('localBrowser.catalogReadUnavailable')));assert.ok(html.includes(t('localBrowser.retryCatalog')));assert.doesNotMatch(html,/localBrowser\./);
 const button=nodes(tree,n=>n.type==='button')[0];assert.equal(button.props.disabled,false);button.props.onClick();assert.equal(clicked,1);
 assert.equal(Notice({catalog:{...state,error:null},available:true,updating:false,onRetry:()=>assert.fail()}),null);
 const unsupported=Notice({catalog:{...state,error:'SOURCE_AVAILABILITY_UNSUPPORTED_FORMAT'},available:true,updating:false,onRetry:()=>assert.fail()});assert.equal(nodes(unsupported,n=>n.type==='button').length,0);
});
test('Local retry is blocked by each exclusive state, including manual check admission and unavailable engine',()=>{
 assert.equal(canRetryLocalCatalog(state,false,true),true);
 for(const key of ['loading','mpdBusy','saveBlocked','manualCheckPlayback'])assert.equal(canRetryLocalCatalog({...state,[key]:true},false,true),false,key);
 assert.equal(canRetryLocalCatalog(state,true,true),false);assert.equal(canRetryLocalCatalog(state,false,false),false);assert.equal(canRetryLocalCatalog({...state,error:null},false,true),false);
});
test('the actual Local retry handler rechecks the current gate, reads only the catalog and retains confirmed rows through failure',async()=>{
 const file=ts.createSourceFile('LocalBrowserView.tsx',read('components/LocalBrowserView.tsx'),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);let callback;
 function visit(node){if(ts.isJsxSelfClosingElement(node)&&node.tagName.getText(file)==='LocalCatalogReadError')callback=node.attributes.properties.find(p=>p.name?.text==='onRetry').initializer.expression.getText(file);ts.forEachChild(node,visit)}visit(file);assert.ok(callback);
 let calls=0,mode='ok',resolve;const known=[{id:'confirmed',title:'Album',artist:'Artist'}];
 const catalog=new LocalAlbumCatalog(async()=>{calls++;if(mode==='fail')throw 'SOURCE_AVAILABILITY_CHANGED';if(mode==='pending')return new Promise(r=>resolve=r);return known;});
 catalog.setUpdating(false,true);await tick();mode='fail';catalog.requestRefresh();await tick();assert.equal(catalog.getSnapshot().albums,known);
 const updating={current:false};const js=ts.transpileModule('const retry='+callback,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 const retry=new Function('canRetryLocalCatalog','localAlbumCatalog','libraryUpdatingRef','isPlaybackAvailable',js+';return retry;')(canRetryLocalCatalog,catalog,updating,true);
 catalog.setSaveBlocked(true);retry();await tick();assert.equal(calls,2);
 catalog.setSaveBlocked(false);await tick();assert.equal(calls,3);updating.current=true;retry();await tick();assert.equal(calls,3);
 updating.current=false;mode='pending';retry();retry();await tick();assert.equal(calls,4);assert.equal(catalog.getSnapshot().albums,known);assert.equal(catalog.getSnapshot().error,null);
 const replacement=[{id:'new'}];resolve(replacement);await tick();assert.equal(catalog.getSnapshot().albums,replacement);assert.equal(catalog.getSnapshot().error,null);
});
