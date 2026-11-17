#!/usr/bin/env python3
"""Offline source preparation / optional cross build. Never runs Windows exes."""
from pathlib import Path, PurePosixPath
import argparse,hashlib,json,os,shutil,subprocess,sys,tarfile
p=argparse.ArgumentParser();p.add_argument('--prepare-only',action='store_true');p.add_argument('--build',action='store_true');p.add_argument('--toolchain',type=Path);args=p.parse_args()
root=Path(__file__).resolve().parent
if not args.prepare_only and not args.build:p.error('choose --prepare-only or --build (no audio is executed)')
manifest=json.loads((root/'SOURCES.json').read_text())
for item in manifest['archives']:
 path=root/item['path'];assert hashlib.sha256(path.read_bytes()).hexdigest()==item['sha256'],item['path']
def extract(archive,destination):
 assert not destination.exists(),f'preserve existing path: {destination}'
 destination.mkdir()
 with tarfile.open(archive) as t:
  tops={PurePosixPath(m.name).parts[0] for m in t.getmembers() if PurePosixPath(m.name).parts}
  assert len(tops)==1,tops
  t.extractall(destination,filter='data')
 return destination/next(iter(tops))
source=extract(root/'archives/mpd.source',root/'mpd-extract')
source.rename(root/'source');(root/'mpd-extract').rmdir();source=root/'source'
subprocess.run(['git','apply','--check',str(root/'windows-audio.patch')],cwd=source,check=True)
subprocess.run(['git','apply',str(root/'windows-audio.patch')],cwd=source,check=True)
cache=source/'subprojects/packagecache';cache.mkdir(exist_ok=True)
for a in manifest['wraps']:shutil.copyfile(root/a['path'],cache/a['filename'])
deps=root/'full-deps';deps.mkdir(exist_ok=True);entries=[]
for item in manifest['dependencies']:
 path=extract(root/item['archive'],deps/(item['name']+'-source'))
 entry=dict(item);entry['source_directory']=str(path);entries.append(entry)
(deps/'sources-verified.json').write_text(json.dumps(entries,indent=2)+'\n')
zlib=next(Path(e['source_directory']) for e in entries if e['name']=='zlib')
subprocess.run(['patch','-p1','--batch','--fuzz=0','-i',str(source/'src/lib/zlib/patches/no_static_suffix')],cwd=zlib,check=True)
for name in ['build-cmake.py','build-codecs.py']:shutil.copyfile(root/name,deps/name)
if args.prepare_only:print('Verified archives, reconstructed MPD Windows patch + pinned dependencies, no build/runtime execution.');sys.exit(0)
assert args.toolchain and (args.toolchain/'bin/x86_64-w64-mingw32-clang').is_file(),'provide the pinned LLVM-MinGW extracted directory'
(root/'toolchain').mkdir();(root/'toolchain'/args.toolchain.name).symlink_to(args.toolchain.resolve(),target_is_directory=True)
(root/'bin').mkdir()
for name in ['ninja','meson']:
 executable=shutil.which(name);assert executable,name;(root/'bin'/name).symlink_to(executable)
for name in ['build-cmake.py','build-codecs.py']:subprocess.run([sys.executable,str(deps/name)],check=True)
tool=args.toolchain.resolve()/'bin';cross=root/'cross.ini'
names={'c':'clang','cpp':'clang++','ar':'ar','strip':'strip','windres':'windres'}
text='[binaries]\n'+''.join(f"{k} = '{tool}/x86_64-w64-mingw32-{v}'\n" for k,v in names.items())
text+="pkg-config = '/usr/bin/pkg-config'\n[host_machine]\nsystem = 'windows'\ncpu_family = 'x86_64'\ncpu = 'x86_64'\nendian = 'little'\n[properties]\nneeds_exe_wrapper = true\n[built-in options]\nc_link_args = ['-static']\ncpp_link_args = ['-static']\n";cross.write_text(text)
env=os.environ.copy();env['PATH']=str(root/'bin')+':'+env['PATH'];env['PKG_CONFIG_LIBDIR']=str(deps/'prefix/lib/pkgconfig');env['PKG_CONFIG_PATH']=''
command=['nice','-n','10','meson','setup',str(root/'build'),str(source),'--cross-file',str(cross),'--buildtype=release','--default-library=static','--wrap-mode=nodownload']+manifest['meson_options']
subprocess.run(command,env=env,check=True);subprocess.run(['nice','-n','10','ninja','-C',str(root/'build'),'-j1'],env=env,check=True)
print('Cross build complete. No Windows executable was run. Byte reproducibility/native audio are separate checks.')
