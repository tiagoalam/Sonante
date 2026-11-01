import React, { useImperativeHandle, useLayoutEffect, useRef } from "react";
import { VirtualAlbumGrid, type VirtualAlbumGridHandle } from "./VirtualAlbumGrid";
import { VirtualList } from "./VirtualList";
import { ALBUM_CARD_WIDTHS, type AlbumPresentation } from "../utils/albumPresentation";
interface Props<T> {
  items: readonly T[]; getItemKey: (item: T) => React.Key; renderItem: (item: T) => React.ReactNode;
  presentation: AlbumPresentation; scrollContainer: HTMLElement | null; resetKey?: string;
}
function AlbumCatalogComponent<T>({presentation, ...props}: Props<T>, ref: React.ForwardedRef<VirtualAlbumGridHandle>) {
  const view=useRef<VirtualAlbumGridHandle>(null);
  const layoutKey=JSON.stringify(presentation), previous=useRef(layoutKey);
  useImperativeHandle(ref,()=>({scrollToIndex:index=>view.current?.scrollToIndex(index)}),[]);
  useLayoutEffect(()=>{
    if(previous.current!==layoutKey){previous.current=layoutKey;view.current?.scrollToIndex(0);}
  },[layoutKey]);
  return presentation.layout==='grid'
    ? <VirtualAlbumGrid {...props} ref={view} minimumCardWidth={ALBUM_CARD_WIDTHS[presentation.coverSize]} />
    : <VirtualList {...props} ref={view} rowHeight={presentation.layout==='list'?80:96} />;
}
export const AlbumCatalog=React.forwardRef(AlbumCatalogComponent) as <T>(props: Props<T> & React.RefAttributes<VirtualAlbumGridHandle>)=>React.ReactElement;
