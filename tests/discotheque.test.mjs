import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { carouselWindow, carouselKeyDirection, carouselSwipeDirection, carouselCoverSize } from '../src/utils/albumCarousel.mjs';
import { albumPresentation, albumPresentationKey, readAlbumPresentation, saveAlbumPresentation } from '../src/utils/albumPresentation.ts';
import { virtualGridMetrics } from '../src/utils/virtualGrid.ts';

test('carousel bounds zero, one and 57 thousand albums to at most five cards', () => {
  assert.deepEqual(carouselWindow(0, 20), {selected:0,start:0,end:0});
  assert.deepEqual(carouselWindow(1, 20), {selected:0,start:0,end:1});
  for (const index of [0,1,100,56999,99999]) { const view=carouselWindow(57000,index); assert.ok(view.end-view.start<=5); assert.ok(view.selected<57000); }
});
test('screenshot-equivalent source widths render the same square center cover', () => {
  const stageHeight = 420;
  const stageWidths = { local: 527, plex: 435, subsonic: 480 };
  const oldPolicy = width => Math.floor(Math.min(350, width * .62, stageHeight - 30));
  assert.equal(oldPolicy(stageWidths.local), 326);
  assert.equal(oldPolicy(stageWidths.plex), 269);
  const rendered = Object.values(stageWidths).map(width => carouselCoverSize(width, stageHeight));
  assert.deepEqual(rendered, [328, 328, 328]);
  assert.equal(carouselCoverSize(437, 301), 271, 'a genuinely shorter stage still scales to fit');
});
test('keyboard and swipe directions require deliberate movement', () => {
  assert.equal(carouselKeyDirection('ArrowLeft'),-1); assert.equal(carouselKeyDirection('ArrowRight'),1); assert.equal(carouselKeyDirection('Enter'),0);
  assert.equal(carouselSwipeDirection(100,30),1); assert.equal(carouselSwipeDirection(30,100),-1); assert.equal(carouselSwipeDirection(100,60),0); assert.equal(carouselSwipeDirection(null,30),0);
});
test('shelf virtualizes rows and retains a finite visible window', () => {
  const m=virtualGridMetrics({itemCount:57000,containerWidth:1000,minimumColumnWidth:170,gap:24,cardExtraHeight:64,scrollOffset:700000,viewportHeight:700,overscanRows:3});
  assert.ok(m.rowCount>1000); assert.ok((m.endRow-m.startRow)*m.columnCount<100); assert.ok(m.startRow>0);
});
test('presentation preferences isolate sources and retain all supported layouts', () => {
  const map=new Map(); const old=globalThis.localStorage; globalThis.localStorage={getItem:key=>map.get(key)??null,setItem:(key,value)=>map.set(key,value)};
  try { for(const source of ['local','plex','navidrome','collections']) { saveAlbumPresentation(source,{layout:'shelf',coverSize:'small'}); assert.equal(readAlbumPresentation(source).layout,'grid'); assert.equal(JSON.parse(map.get(albumPresentationKey(source))).layout,'shelf'); }
    saveAlbumPresentation('collections',{layout:'carousel',coverSize:'medium'}); assert.equal(readAlbumPresentation('collections').layout,'carousel'); assert.equal(readAlbumPresentation('local').layout,'grid'); assert.notEqual(albumPresentationKey('local'),albumPresentationKey('collections')); assert.equal(albumPresentation({layout:'other'}).layout,'grid');
  } finally { globalThis.localStorage=old; }
});
test('shared integration keeps existing source cards and collection availability checks', () => {
  const read=path=>readFileSync(new URL('../src/'+path,import.meta.url),'utf8');
  const catalog=read('components/AlbumCatalog.tsx'), collection=read('components/PersonalCollectionsView.tsx'), carousel=read('components/AlbumCarousel.tsx');
  assert.match(catalog,/AlbumCarousel/); assert.match(catalog,/VirtualAlbumGrid/); assert.match(collection,/disabled=\{!canOpen\}/); assert.match(collection,/data-collection-member/); assert.match(collection,/MutationObserver/); assert.match(carousel,/prefers|discotheque\.css/); assert.match(read('components/discotheque.css'),/prefers-reduced-motion/);
  assert.match(read('components/NavidromeView.tsx'),/onReachEnd=.*feed\.more/);
  assert.match(read('components/LocalBrowserView.tsx'),/preferFullArtwork=\{carousel\?\.focused\}/);
  assert.match(read('components/PersonalCollectionArtwork.tsx'),/undefined, current, false, preferFullArtwork/);
  assert.match(read('components/VirtualAlbumGrid.tsx'),/shelf \? <div className="album-shelf-item">/);
  assert.match(read('components/discotheque.css'),/album-shelf-row::before/);
});

test('carousel renders a bounded interactive center and responds to keyboard and swipe', async () => {
  const React=(await import('react')).default;
  const jsx=await import('react/jsx-runtime');
  const icons=await import('lucide-react');
  const ts=(await import('typescript')).default;
  const util=await import('../src/utils/albumCarousel.mjs');
  let value=0, hookIndex=0, layoutEffect, dimensions; const refs=[]; const hooks={...React,forwardRef:fn=>fn,useState:initial=>hookIndex++ === 0 ? [value,next=>{value=typeof next==='function'?next(value):next;}] : [initial,next=>{dimensions=next(initial);} ],useRef:()=>{const ref={current:null};refs.push(ref);return ref;},useEffect:()=>{},useLayoutEffect:fn=>{layoutEffect=fn;},useImperativeHandle:()=>{}};
  const dependencies={react:hooks,'react/jsx-runtime':jsx,'lucide-react':icons,'react-i18next':{useTranslation:()=>({t:key=>key})},'../utils/albumCarousel.mjs':util,'./discotheque.css':{}};
  const source=readFileSync(new URL('../src/components/AlbumCarousel.tsx',import.meta.url),'utf8');
  const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.CommonJS,jsx:ts.JsxEmit.ReactJSX,target:ts.ScriptTarget.ES2022}}).outputText;
  const exports={};new Function('require','exports',code)(name=>{assert.ok(name in dependencies,name);return dependencies[name];},exports);
  let opened=0, loads=0; const focusStates=[]; const items=Array.from({length:20},(_,id)=>({id,title:`Album ${id}`,artist:'Artist',year:2001,track_count:10,source:'local'}));
  const paint=(scrollContainer)=>{hookIndex=0; refs.length=0; return exports.AlbumCarousel({scrollContainer,items,getItemKey:item=>item.id,renderItem:(item,context)=>{focusStates.push({id:item.id,focused:context.focused});return jsx.jsx('button',{onClick:()=>{opened=item.id+1;},children:String(item.id)});},onReachEnd:()=>loads++},null);};
  let tree=paint(); let stage=tree.props.children[1]; assert.equal(stage.props.children.length,3); assert.equal(stage.props.children.filter(node=>!node.props.children.props.inert).length,1); assert.equal(focusStates.filter(item=>item.focused).length,1);
  const focus={};tree.props.onKeyDown({target:focus,currentTarget:focus,key:'ArrowRight',preventDefault(){},stopPropagation(){}});assert.equal(value,1);
  tree=paint();const panel=tree.props.children[0];assert.equal(panel.props.children[1].props.children,'Album 1');assert.equal(panel.props.children[2].props.children,'Artist'); assert.equal(panel.props.children[0].props.children,'collections.sources.local'); assert.equal(panel.props.children[3].props.children[0],'2001'); assert.equal(panel.props.children[3].props.children[2],'albumViews.trackCount');
  const input={closest:()=>({})};tree.props.onKeyDown({target:input,currentTarget:focus,key:'ArrowRight',preventDefault(){assert.fail('input must not be intercepted')}});assert.equal(value,1);
  const button={closest:()=>null};tree.props.onKeyDown({target:button,currentTarget:focus,key:'ArrowLeft',preventDefault(){assert.fail('nested control')}});assert.equal(value,1);tree.props.onKeyDown({target:focus,currentTarget:focus,key:'ArrowLeft',preventDefault(){},stopPropagation(){}});assert.equal(value,0);tree=paint();tree.props.onKeyDown({target:focus,currentTarget:focus,key:'ArrowRight',preventDefault(){},stopPropagation(){}});assert.equal(value,1);
  let keyboardOpens=0; refs[1].current={querySelector:()=>({matches:()=>false,getAttribute:()=>null,click:()=>keyboardOpens++})}; tree.props.onKeyDown({target:focus,currentTarget:focus,key:'Enter',preventDefault(){}}); assert.equal(keyboardOpens,1); tree.props.onKeyDown({target:button,currentTarget:focus,key:'Enter',preventDefault(){assert.fail('nested control')}}); assert.equal(keyboardOpens,1); refs[1].current={querySelector:()=>({matches:()=>true})}; tree.props.onKeyDown({target:focus,currentTarget:focus,key:'Enter',preventDefault(){assert.fail('disabled')}}); assert.equal(keyboardOpens,1);
  tree=paint();tree.props.onTouchStart({touches:[{clientX:150}]});tree.props.onTouchEnd({changedTouches:[{clientX:30}]});assert.equal(value,2);
  let prevented=false, stopped=false; tree.props.onClickCapture({preventDefault(){prevented=true;},stopPropagation(){stopped=true;}}); assert.equal(prevented,true); assert.equal(stopped,true);
  tree=paint();stage=tree.props.children[1];assert.equal(stage.props.children.length,5);const center=stage.props.children.find(node=>node.props['data-position']===0);center.props.children.props.children.props.onClick();assert.equal(opened,3);assert.equal(loads,0);
  const oldObserver=globalThis.ResizeObserver, oldStyle=globalThis.getComputedStyle;
  try {
    let update; globalThis.ResizeObserver=class { constructor(fn){update=fn;} observe(){} disconnect(){} };
    globalThis.getComputedStyle=()=>({paddingBottom:'32px'});
    const container={clientHeight:720,getBoundingClientRect:()=>({top:100})};
    paint(container); refs[0].current={getBoundingClientRect:()=>({top:200})}; refs[1].current={clientWidth:1000,clientHeight:400};
    const cleanup=layoutEffect(); assert.deepEqual(dimensions,{height:588,cover:328});
    container.clientHeight=400; refs[1].current.clientHeight=200; update(); assert.deepEqual(dimensions,{height:268,cover:170}); cleanup();
  } finally { globalThis.ResizeObserver=oldObserver; globalThis.getComputedStyle=oldStyle; }

});

test('focused carousel arrows are consumed before App playback shortcuts while shortcuts remain global elsewhere', async () => {
  const React = (await import('react')).default;
  const jsx = await import('react/jsx-runtime');
  const icons = await import('lucide-react');
  const ts = (await import('typescript')).default;
  const util = await import('../src/utils/albumCarousel.mjs');
  const appSource = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
  const start = appSource.indexOf('const handleKeyDown = (e: KeyboardEvent) => {');
  const end = appSource.indexOf('window.addEventListener("keydown", handleKeyDown)', start);
  assert.ok(start > 0 && end > start, 'the real App playback listener is present');
  assert.match(appSource.slice(end, end + 80), /window\.addEventListener\("keydown", handleKeyDown\);/,
    'the playback shortcut listens during bubbling, after the focused carousel');
  const globalCode = ts.transpileModule(appSource.slice(start, end), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;

  let selected = 0, hookIndex = 0;
  const hooks = {
    ...React,
    forwardRef: fn => fn,
    useState(initial) {
      const slot = hookIndex++;
      return slot === 0
        ? [selected, next => { selected = typeof next === 'function' ? next(selected) : next; }]
        : [initial, () => {}];
    },
    useRef: () => ({ current: null }),
    useEffect: () => {},
    useLayoutEffect: () => {},
    useImperativeHandle: () => {},
  };
  const dependencies = {
    react: hooks,
    'react/jsx-runtime': jsx,
    'lucide-react': icons,
    'react-i18next': { useTranslation: () => ({ t: key => key }) },
    '../utils/albumCarousel.mjs': util,
    './discotheque.css': {},
  };
  const componentSource = readFileSync(new URL('../src/components/AlbumCarousel.tsx', import.meta.url), 'utf8');
  const componentCode = ts.transpileModule(componentSource, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  new Function('require', 'exports', componentCode)((name) => {
    assert.ok(name in dependencies, name);
    return dependencies[name];
  }, exports);

  class FocusElement {
    constructor(tagName = 'SECTION', dialog = false) { this.tagName = tagName; this.dialog = dialog; }
    closest(selector) {
      if (this.dialog && selector.includes('[role="dialog"]')) return this;
      if (this.tagName === 'INPUT' && selector.startsWith('input,')) return this;
      return null;
    }
  }
  const root = new FocusElement();
  const document = { activeElement: root, querySelector: () => null };
  const calls = [];
  const playbackHandler = new Function(
    'document', 'HTMLElement', 'nowPlayingHosting', 'playbackAvailableRef', 'statusRef',
    'audioService', 'playerNavigationRequests', 'logError', 'window',
    `const config=null, showSettings=false, showAbout=false, showQueue=false, searchQuery="";
     const playerTarget=null, exploreDetail=null, discoverAlbum=null, discoverArtist=null;
     const centralSection="local", plexArea="library", plexDiscover={getSnapshot:()=>({})};
     const closePlayerNavigation=()=>{}, closeExploreAlbum=()=>{}, closeDiscoverArtist=()=>{};
     const setDiscoverAlbum=()=>{}, setShowAbout=()=>{}, setShowSettings=()=>{}, setShowQueue=()=>{};
     const setLocalFoldersDraft=()=>{}, setSearchQuery=()=>{}, mainSectionPreference={navigate(){}};
     const resetAllNavigation=()=>{}, searchInputRef={current:null}, canToggleMute=()=>false;
     const directMuteRequest=()=>null, sharedMuteRequest=()=>null, t=value=>value;
     ${globalCode}; return handleKeyDown;`
  )(
    document, FocusElement, { getSnapshot: () => ({ mode: 'closed' }) }, { current: true }, { current: {} },
    {
      next: () => { calls.push('next'); return Promise.resolve(); },
      previous: () => { calls.push('previous'); return Promise.resolve(); },
      togglePlay: () => { calls.push('toggle'); return Promise.resolve(); },
    },
    { invalidate: () => {} }, () => {}, {},
  );
  const items = Array.from({ length: 20 }, (_, id) => ({ id, title: `Album ${id}` }));
  const paint = () => {
    hookIndex = 0;
    return exports.AlbumCarousel({ items, getItemKey: item => item.id, renderItem: item => jsx.jsx('button', { children: item.title }) }, null);
  };
  const dispatchFromCarousel = (tree, key) => {
    const event = {
      key, code: key, target: root, currentTarget: root, defaultPrevented: false, cancelBubble: false,
      ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, nativeEvent: { isComposing: false },
      preventDefault() { this.defaultPrevented = true; },
      stopPropagation() { this.cancelBubble = true; },
    };
    tree.props.onKeyDown(event);
    if (!event.cancelBubble) playbackHandler(event);
    return event;
  };

  for (let i = 0; i < 10; i++) {
    const event = dispatchFromCarousel(paint(), 'ArrowRight');
    assert.equal(event.defaultPrevented, true);
    assert.equal(event.cancelBubble, true, 'a handled carousel arrow must not reach the window listener');
  }
  for (let i = 0; i < 3; i++) dispatchFromCarousel(paint(), 'ArrowLeft');
  assert.equal(selected, 7, 'both directions move only the selected album');
  assert.deepEqual(calls, [], 'carousel navigation sends no playback command');

  const outside = new FocusElement('BUTTON');
  document.activeElement = outside;
  const outsideEvent = {
    key: 'ArrowRight', code: 'ArrowRight', target: outside, defaultPrevented: false,
    ctrlKey: false, altKey: false, metaKey: false, shiftKey: false,
    preventDefault() { this.defaultPrevented = true; },
  };
  playbackHandler(outsideEvent);
  assert.deepEqual(calls, ['next'], 'the normal global shortcut still works outside the carousel');
  playbackHandler({
    ...outsideEvent, key: 'ArrowLeft', code: 'ArrowLeft',
    preventDefault() { this.defaultPrevented = true; },
  });
  assert.deepEqual(calls, ['next', 'previous'], 'the previous-track shortcut also remains available outside the carousel');

  const input = new FocusElement('INPUT');
  document.activeElement = input;
  const inputEvent = {
    key: 'ArrowRight', code: 'ArrowRight', target: input, defaultPrevented: false,
    ctrlKey: false, altKey: false, metaKey: false, shiftKey: false,
    preventDefault() { this.defaultPrevented = true; },
  };
  playbackHandler(inputEvent);
  assert.deepEqual(calls, ['next', 'previous'], 'editable fields retain their current keyboard behavior');

  const dialog = new FocusElement('DIV', true);
  document.activeElement = dialog;
  const tree = paint();
  const dialogEvent = {
    key: 'ArrowRight', code: 'ArrowRight', target: dialog, currentTarget: root, defaultPrevented: false, cancelBubble: false,
    ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, nativeEvent: { isComposing: false },
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.cancelBubble = true; },
  };
  tree.props.onKeyDown(dialogEvent);
  assert.equal(selected, 7, 'the carousel does not navigate for a dialog target');
  assert.equal(dialogEvent.defaultPrevented, false);
  assert.equal(dialogEvent.cancelBubble, false);
});

test('carousel sizing uses the available scroll area and preserves responsive overflow', () => {
  const component=readFileSync(new URL('../src/components/AlbumCarousel.tsx',import.meta.url),'utf8');
  const css=readFileSync(new URL('../src/components/discotheque.css',import.meta.url),'utf8');
  assert.match(component,/scrollContainer\.clientHeight - offset - padding/);
  assert.match(component,/carouselCoverSize\(stage\.current\.clientWidth, stage\.current\.clientHeight, cardExtraHeight\)/);
  assert.match(css,/width: var\(--carousel-cover-size, min\(328px, 100%\)\)/);
  assert.match(css,/min-height: 450px/);
  assert.doesNotMatch(css,/\.album-carousel \{[^}]*overflow:\s*hidden/);
  assert.match(css,/\.album-carousel:focus-visible/);
  for (const file of ['App.tsx','components/LocalBrowserView.tsx','components/NavidromeView.tsx','components/PersonalCollectionsView.tsx']) assert.match(readFileSync(new URL('../src/'+file,import.meta.url),'utf8'),/data-album-open/);
});
