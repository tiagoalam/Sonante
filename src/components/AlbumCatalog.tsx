import React, { useImperativeHandle, useLayoutEffect, useRef } from "react";
import { VirtualAlbumGrid, type VirtualAlbumGridHandle } from "./VirtualAlbumGrid";
import { AlbumCarousel } from "./AlbumCarousel";
import { VirtualList } from "./VirtualList";
import { ALBUM_CARD_WIDTHS, type AlbumPresentation } from "../utils/albumPresentation";
interface Props<T> {
  items: readonly T[]; getItemKey: (item: T) => React.Key; renderItem: (item: T, context?: { focused: boolean }) => React.ReactNode; sourceLabel?: string; cardExtraHeight?: number;
  presentation: AlbumPresentation; scrollContainer: HTMLElement | null; resetKey?: string; onReachEnd?: () => void;
}
function AlbumCatalogComponent<T>({presentation, ...props}: Props<T>, ref: React.ForwardedRef<VirtualAlbumGridHandle>) {
  const view=useRef<VirtualAlbumGridHandle>(null);
  const layoutKey=JSON.stringify(presentation), previous=useRef(layoutKey);
  useImperativeHandle(ref,()=>({scrollToIndex:index=>view.current?.scrollToIndex(index)}),[]);
  useLayoutEffect(()=>{
    if(previous.current!==layoutKey){previous.current=layoutKey;view.current?.scrollToIndex(0);}
  },[layoutKey]);
  return presentation.layout==='carousel' ? <AlbumCarousel {...props} ref={view} /> : presentation.layout==='grid' || presentation.layout==='shelf'
    ? <VirtualAlbumGrid {...props} ref={view} shelf={presentation.layout==='shelf'} minimumCardWidth={presentation.layout === 'shelf' ? Math.max(190, ALBUM_CARD_WIDTHS[presentation.coverSize]) : ALBUM_CARD_WIDTHS[presentation.coverSize]} />
    : <VirtualList {...props} ref={view} rowHeight={presentation.layout==='list'?80:96} />;
}
export const AlbumCatalog=React.forwardRef(AlbumCatalogComponent) as <T>(props: Props<T> & React.RefAttributes<VirtualAlbumGridHandle>)=>React.ReactElement;
