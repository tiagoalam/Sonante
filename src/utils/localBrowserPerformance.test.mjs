import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import ts from 'typescript';
import {sortLocalAlbums} from './localAlbumNavigation.ts';
import {filterLocalAlbumsByScope} from './localLibraryScope.ts';

// Evaluate the browser's real memo initializer with inert hooks and album data.
const source=readFileSync(new URL('../components/LocalBrowserView.tsx',import.meta.url),'utf8');
const tree=ts.createSourceFile('browser.tsx',source,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
let expression;
function visit(node) {
  if(ts.isVariableDeclaration(node)&&node.name.getText(tree)==='sortedAlbums')expression=node.initializer.getText(tree);
  ts.forEachChild(node,visit);
}
visit(tree);
assert.ok(expression);
const code=ts.transpileModule(`return (${expression});`,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
const visibleAlbums=scope=>new Function(...Object.keys(scope),code)(...Object.values(scope));
const albums=Object.freeze(Array.from({length:40},(_,i)=>Object.freeze({
  id:`id-${i}`,title:['Álbum','Album','The Album','Same',''][i%5],artist:`Artist ${i%3}`,
  year:i%4?String(1990+i%5):'',source_id:`source-${i%2}`,folder_path:`fixture/${i}`,track_count:12,
})));
const openingAlbums=sortLocalAlbums(albums,'album-asc');

test('default browser opening reuses album order without a second catalog sort',()=>{
  const result=visibleAlbums({useMemo:fn=>fn(),sortLocalAlbums:()=>assert.fail('duplicate opening sort'),
    filteredAlbums:filterLocalAlbumsByScope(albums,null,''),openingAlbums,albumSortMode:'album-asc',activeSourceId:null,albumSearch:''});
  assert.equal(result,openingAlbums);
  assert.deepEqual(result,sortLocalAlbums(albums,'album-asc'));
});

test('opening order reuse preserves all source, search and alternate sort results',()=>{
  for(const mode of ['album-asc','album-desc','artist-asc','artist-desc','year-newest','year-oldest'])
    for(const source of [null,'source-1','missing'])for(const query of ['','same','Álbum','missing']) {
      const filtered=filterLocalAlbumsByScope(albums,source,query);
      const result=visibleAlbums({useMemo:fn=>fn(),sortLocalAlbums,filteredAlbums:filtered,openingAlbums,
        albumSortMode:mode,activeSourceId:source,albumSearch:query});
      assert.deepEqual(result,sortLocalAlbums(filtered,mode));
      assert.ok(result.every(album=>albums.includes(album)),'album identities reach playback unchanged');
    }
});
