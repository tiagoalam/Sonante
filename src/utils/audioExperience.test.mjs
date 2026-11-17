import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import ts from "typescript";
import React from "react";
import * as jsx from "react/jsx-runtime";
import i18next from "i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { acceptsAudioRouteGeneration } from "./audioRoute.ts";

const i18n=i18next.createInstance();
await i18n.init({lng:"en",resources:Object.fromEntries(["en","pt-BR"].map(language=>[language,{translation:JSON.parse(readFileSync(new URL(`../locales/${language}.json`,import.meta.url),"utf8"))}]))});
function compile(file,dependencies){
 const source=readFileSync(new URL(`../${file}`,import.meta.url),"utf8");const code=ts.transpileModule(source,{compilerOptions:{jsx:ts.JsxEmit.ReactJSX,module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2020}}).outputText;const exports={};new Function("require","exports",code)(id=>{assert.ok(id in dependencies,id);return dependencies[id]},exports);return exports;
}
const helper=compile("utils/audioExperience.ts",{});
const {AudioRouteStatus}=compile("components/AudioRouteStatus.tsx",{
 "react/jsx-runtime":jsx,"react-i18next":{useTranslation:()=>({t:i18n.t.bind(i18n)})},"lucide-react":{AlertCircle:()=>null,CheckCircle2:()=>null,CircleHelp:()=>null,LoaderCircle:()=>null},
 "../utils/audioExperience":helper,
});
const shared={destination:{kind:"system_default"},access:"shared",processing:"off"};
const direct={destination:{kind:"device",deviceId:"hw:CARD=Fixture,DEV=0"},access:"exclusive",processing:"off"};
const equalized={...shared,processing:"equalizer"};
function projection({generation=4,operation="stable",state="confirmed",requested=shared,confirmed=shared,diagnostic=null,evidence=[],pcmAcquired=null,exclusiveAccessConfirmed=null}={}){
 return{generation,operationId:"private-operation-id",operation,route:{state,requested,confirmed},applied:confirmed,evidence,diagnostic,pcmAcquired,exclusiveAccessConfirmed,legacyIssues:[]};
}
function render(value,{selectedMode="shared",pendingSave=false,playing=false,outputObservation=null}={}){
 return renderToStaticMarkup(React.createElement(AudioRouteStatus,{projection:value,selectedMode,pendingSave,playing,outputObservation}));
}

test("status stays short for requested, transitioning, confirmed and uncertain operations",()=>{
 for(const [value,state,message]of[
  [projection({state:"requested",confirmed:null,requested:equalized}),"requested","Output configured"],
  [projection({operation:"transitioning",state:"transitioning",requested:direct}),"transitioning","Changing output"],
  [projection({confirmed:equalized}),"confirmed","Shared output"],
  [projection({operation:"uncertain",state:"uncertain",confirmed:null,requested:direct}),"uncertain","Could not confirm output"],
 ]){
  const html=render(value,{selectedMode:state==="confirmed"?"shared":"direct"});assert.match(html,new RegExp(`data-status="${state}"`));assert.ok(html.includes(message));
  assert.doesNotMatch(html,/Selected mode|Confirmed route|operation-id|pipewire:sink|PCM acquisition was observed/);
 }
});

test("failure and recovery use one short message with optional collapsed details",()=>{
 const failed=render(projection({operation:"failed",state:"uncertain",confirmed:null,diagnostic:"supervisor_failed"}));
 assert.match(failed,/Could not change output/);assert.match(failed,/role="alert"/);assert.match(failed,/<details/);assert.doesNotMatch(failed,/<details open/);
 const recovered=render(projection({operation:"failed",confirmed:shared,diagnostic:"configuration_failed"}));
 assert.match(recovered,/Could not change output/);assert.match(recovered,/previous output was kept/i);assert.doesNotMatch(recovered,/Shared output/);
});

test("Direct without PCM while stopped is configured and awaits playback evidence",()=>{
 const pending=render(projection({confirmed:direct}),{selectedMode:"shared",pendingSave:true});
 assert.match(pending,/Save to apply this change/);assert.doesNotMatch(pending,/Shared output|Exclusive output/);
 const directNoPcm=projection({state:"requested",requested:direct,confirmed:null,diagnostic:"direct_pcm_not_observed"});
 const directHtml=render(directNoPcm,{selectedMode:"direct",playing:false,outputObservation:{state:"unverified",reason:"not_playing"}});
 assert.match(directHtml,/Direct is configured; physical confirmation requires playback/);
 assert.doesNotMatch(directHtml,/Exclusive output|failed|Could not change|PCM acquisition was observed/i);
 assert.doesNotMatch(directHtml,/role="alert"/);
 assert.equal(render(null),"");
});

test("Direct confirmation requires owned-PCM evidence during playback; pending evidence stays uncertain",()=>{
 const confirmed=projection({state:"confirmed",requested:direct,confirmed:direct,evidence:["direct_pcm_observed"],pcmAcquired:true,exclusiveAccessConfirmed:true});
 const confirmedHtml=render(confirmed,{selectedMode:"direct",playing:true,outputObservation:{state:"observed",configured_endpoint:direct.destination.deviceId,card:2,device:0,subdevice:0,format:"S32_LE",rate:96000,channels:2}});
 assert.match(confirmedHtml,/Exclusive output/);assert.match(confirmedHtml,/data-status="confirmed"/);
 const requested=projection({state:"requested",requested:direct,confirmed:null,diagnostic:"direct_pcm_not_observed"});
 const activeHtml=render(requested,{selectedMode:"direct",playing:true,outputObservation:{state:"unverified",reason:"not_owned"}});
 assert.match(activeHtml,/Could not confirm output/);assert.match(activeHtml,/data-status="uncertain"/);
 assert.doesNotMatch(activeHtml,/Exclusive output|Could not change output/);
 const observedButOlderRoute=render(requested,{selectedMode:"direct",playing:true,outputObservation:{state:"observed",configured_endpoint:direct.destination.deviceId,card:2,device:0,subdevice:0,format:"S32_LE",rate:96000,channels:2}});
 assert.match(observedButOlderRoute,/Confirming output/);assert.match(observedButOlderRoute,/data-status="confirming"/);
});

test("transition and genuine transition failure remain distinguishable",()=>{
 const pending=render(projection({operation:"transitioning",state:"transitioning",requested:direct,confirmed:shared}),{selectedMode:"direct"});
 assert.match(pending,/Changing output/);assert.match(pending,/data-status="transitioning"/);assert.doesNotMatch(pending,/Could not confirm output/);
 const failed=render(projection({operation:"failed",state:"uncertain",requested:direct,confirmed:null,diagnostic:"configuration_failed"}),{selectedMode:"direct"});
 assert.match(failed,/Could not change output/);assert.match(failed,/role="alert"/);assert.match(failed,/<details/);
});

test("a late older route snapshot cannot replace a newer confirmed Direct route",()=>{
 let latestGeneration=11;
 let visible=projection({generation:11,state:"requested",requested:direct,confirmed:null,diagnostic:"direct_pcm_not_observed"});
 const newer=projection({generation:12,state:"confirmed",requested:direct,confirmed:direct,evidence:["direct_pcm_observed"],pcmAcquired:true,exclusiveAccessConfirmed:true});
 if(acceptsAudioRouteGeneration(newer.generation,latestGeneration)){latestGeneration=newer.generation;visible=newer;}
 const stale=projection({generation:11,state:"requested",requested:direct,confirmed:null,diagnostic:"direct_pcm_not_observed"});
 if(acceptsAudioRouteGeneration(stale.generation,latestGeneration)){latestGeneration=stale.generation;visible=stale;}
 assert.equal(latestGeneration,12);assert.match(render(visible,{selectedMode:"direct",playing:true}),/Exclusive output/);
});

test("EQ page availability is based only on a verified runtime or confirmed route snapshot",()=>{
 const {equalizerIsActive,latestEqualizerSnapshot}=helper;
 assert.equal(equalizerIsActive(null,null),false);
 assert.equal(equalizerIsActive({...projection(),config:{enabled:true}}),false);
 assert.equal(equalizerIsActive({phase:"preparing",verified:false,in_path:null}),false);
 assert.equal(equalizerIsActive({phase:"active",verified:true,in_path:true}),true);
 assert.equal(equalizerIsActive({phase:"ready",verified:true,in_path:true,comparison:{side:"reference"}}),true);
 const applied={id:"fixture",bands:[]};
 assert.equal(equalizerIsActive({phase:"updating",revision:8,verified:true,in_path:true,applied}),true,"verified route stays active while an applied preset is changing");
 assert.equal(equalizerIsActive({phase:"updating",revision:8,verified:false,in_path:true,applied}),false,"an unverified update must not claim an active DSP");
 assert.equal(equalizerIsActive({phase:"error",revision:9,verified:false,in_path:false,applied,error:"runtime_failed"}),false,"a real DSP failure must remain inactive");
 const newerUpdate={phase:"updating",revision:12,verified:true,in_path:true,applied};
 const staleOff={phase:"off",revision:11,verified:false,in_path:false,applied:null};
 assert.equal(latestEqualizerSnapshot(staleOff,newerUpdate),newerUpdate,"late older Off snapshots cannot hide a newer active preset application");
 const newerFailure={phase:"error",revision:13,verified:false,in_path:false,applied:null,error:"runtime_failed"};
 assert.equal(latestEqualizerSnapshot(newerUpdate,newerFailure),newerFailure,"the newest actual failure takes precedence");
 assert.equal(equalizerIsActive(latestEqualizerSnapshot(newerUpdate,newerFailure)),false);
 const confirmedProjection=projection({confirmed:equalized});
 assert.equal(equalizerIsActive(null,confirmedProjection),true);
 assert.equal(equalizerIsActive(null,{...confirmedProjection,route:{...confirmedProjection.route,state:"uncertain",confirmed:null}}),false);
});
