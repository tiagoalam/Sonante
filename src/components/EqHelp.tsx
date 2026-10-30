import type {ReactNode} from 'react';
import {useTranslation} from 'react-i18next';
import {SignalPathInfo} from './SignalPath';
/** Reuse the existing keyboard/click native-dialog help, including focus return. */
export function EqHelp({title,children}:{title:string;children:ReactNode}) {
 const{t}=useTranslation();
 return <span className="contents" onKeyDownCapture={event=>{
  if(event.key!=='Tab')return;
  const dialog=(event.target as HTMLElement).closest('dialog');if(!dialog)return;
  const items=[...dialog.querySelectorAll<HTMLElement>('button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex="0"]')].filter(e=>e.getClientRects().length>0);
  const first=items[0],last=items[items.length-1];
  if(first&&last&&(event.shiftKey?document.activeElement===first:document.activeElement===last)){event.preventDefault();(event.shiftKey?last:first).focus()}
 }}><SignalPathInfo title={title} openLabel={t('equalizer.helpFor',{topic:title})} closeLabel={t('equalizer.closeHelp')}><div className="eq-help-content">{children}</div></SignalPathInfo></span>;
}
