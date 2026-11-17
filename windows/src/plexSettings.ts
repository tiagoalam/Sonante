import type {AppConfig} from '../../src/types/config';
type PublicAccount=Pick<AppConfig,'plex_token'>&{windows_plex_saved?:boolean;windows_plex_revision?:string|null;plex_server_id?:string|null};
export function plexAccountIdentity(config:PublicAccount|null):string{
 if(!config)return "";
 return config.windows_plex_saved?(config.windows_plex_revision??config.plex_server_id??""):config.plex_token;
}
// A real public status bit, never a token sentinel or authenticated URI.
export function hasPlexAccount(config:AppConfig):boolean{
 return Boolean(config.plex_token.trim()||(config as AppConfig&{windows_plex_saved?:boolean}).windows_plex_saved);
}
