// Preserve the original wizard. Saved Plex presence is a public native binding,
// never a fabricated credential. New PIN credentials keep their original flow.
export function adaptWelcomeWizard(code:string,helpersPath:string):string{
 const changes=[
  ['onFinish(finalConfig);','onFinish(await configService.getConfig());'],
  ['{!config.plex_token ? (','{!hasPlexAccount(config) ? ('],
  ['{ ...prev, plex_token: "", plex_url: "", plex_server_id: null, plex_server_name: null }','{ ...prev, ...{windows_plex_saved:false,windows_plex_revision:null}, plex_token: "", plex_url: "", plex_server_id: null, plex_server_name: null }'],
 ];
 for(const [before,after] of changes){if(code.split(before).length!==2)throw new Error('Windows welcome wizard contract changed');code=code.replace(before,after);}
 return `import {hasPlexAccount} from ${JSON.stringify(helpersPath)};\n${code}`;
}
