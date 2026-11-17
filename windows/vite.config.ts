import {defineConfig} from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import {fileURLToPath} from "node:url";
import path from "node:path";
import {readFileSync} from "node:fs";
import {adaptWelcomeWizard} from "./src/wizardTransform.ts";
const windows=fileURLToPath(new URL(".",import.meta.url));
const shared=path.resolve(windows,"../src");
const milestones=[...readFileSync(path.resolve(windows,"../src-tauri/src/windows.rs"),"utf8").matchAll(/milestone:\s*"windows-shared-ui-preview-([1-9]\d*)"/g)];
if(milestones.length!==1)throw new Error("Windows preview milestone must have exactly one source");
const previewLabel=`Windows Preview ${milestones[0][1]}`;
const adapted=new Set(["SidebarNavItem","SettingsPages","SourcesSettings","AudioSignalDiagram","SignalPath","RemoteControlSetting","UpnpSetting","ExperimentalFeatures","AlbumABComparison","PlayerBar","NowPlayingHost","LocalWatchNotice","LocalBrowserView","SmartPlaylists"]);
export default defineConfig({
 root:windows,
 plugins:[{name:"sonante-windows-capabilities",enforce:"pre",resolveId(source,importer){
  if(!importer?.startsWith(shared+path.sep)||!source.startsWith("."))return null;
  const target=path.resolve(path.dirname(importer),source).replace(/\.tsx?$/,"");
  if(target===path.join(shared,"utils/albumEditionArchive"))return path.join(windows,"src/editionArchive.ts");
  if(target===path.join(shared,"utils/musicSources"))return path.join(windows,"src/musicSources.ts");
  if(target===path.join(shared,"services/smartPlaylists"))return path.join(windows,"src/smartPlaylists.ts");
  if(target===path.join(shared,"utils/usePlayerSourceInfo"))return path.join(windows,"src/usePlayerSourceInfo.ts");
  if(path.dirname(target)===path.join(shared,"components")&&adapted.has(path.basename(target)))return path.join(windows,"src/adapters.tsx");
  return null;
 },transform(code,id){
  if(id===path.join(windows,"src/main.tsx")){
   const marker="__SONANTE_WINDOWS_PREVIEW_LABEL__";
   if(code.split(marker).length!==2)throw new Error("Windows preview label contract changed");
   return {code:code.replace(marker,previewLabel),map:null};
  }
  if(id===path.join(shared,"components/WelcomeWizard.tsx"))return {code:adaptWelcomeWizard(code,path.join(windows,"src/plexSettings.ts")),map:null};
  if(id===path.join(shared,"components/AboutModal.tsx")){
   const footer="Linux • MPD • ALSA";
   if(code.split(footer).length!==2)throw new Error("Windows About footer contract changed");
   return {code:code.replace(footer,"Windows • MPD • WASAPI"),map:null};
  }
  if(id===path.join(shared,"components/QueueInsertionMenu.tsx")){
   const changes=[['notice: shown ? <p role=','notice: shown ? createPortal(<p role='],['className="mx-4 my-2 rounded-lg','className="windows-queue-feedback mx-4 my-2 rounded-lg'],['</p> : null,','</p>, document.body) : null,']];
   for(const [before,after] of changes){if(code.split(before).length!==2)throw new Error("Windows queue feedback contract changed");code=code.replace(before,after);}
   return {code,map:null};
  }
  const accountReaders=new Set(["App.tsx","components/UnifiedSearchView.tsx","utils/useSourceReadiness.ts","utils/appBootstrap.ts","utils/exploreNavigation.ts","utils/welcomeWizard.ts","services/albumEditions.ts"]);
  if(accountReaders.has(path.relative(shared,id))){
   // These original reads use the credential only as account presence/cache
   // identity. Use the real public binding generation, never a fake token.
   code=code.replace(/(state\.)?config(?:\?\.|\.)plex_token/g,(_,prefix)=>`plexAccountIdentity(${prefix??""}config)`);
   return {code:`import {plexAccountIdentity} from ${JSON.stringify(path.join(windows,"src/plexSettings.ts"))};\n${code}`,map:null};
  }
  if(id!==path.join(shared,"components/SettingsModal.tsx"))return null;
  // Keep the original component and render tree. Windows retains the saved
  // credential in DPAPI; only its public presence participates in this draft.
  const changes=[
   ['cfg.plex_token && cfg.plex_token.trim().length > 0','hasPlexAccount(cfg)'],
   ['Boolean(config.plex_token && config.plex_token.trim().length > 0)','hasPlexAccount(config)'],
   ['      plex_token: "",','      ...{windows_plex_saved:false},\n      plex_token: "",'],
  ];
  for(const [before,after] of changes){if(code.split(before).length!==2)throw new Error("Windows Plex settings contract changed");code=code.replace(before,after);}
  return {code:`import {hasPlexAccount} from ${JSON.stringify(path.join(windows,"src/plexSettings.ts"))};\n${code}`,map:null};
 }},react(),tailwindcss()],
 resolve:{alias:[{find:/^@tauri-apps\/api\/core$/,replacement:path.join(windows,"src/core.ts")}]},
 clearScreen:false,server:{port:1421,strictPort:true},build:{outDir:"dist",emptyOutDir:true}
});
