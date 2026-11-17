from pathlib import Path
import subprocess,json,os,sys
w=Path(__file__).resolve().parent;root=w.parent
sources={r['name']:r for r in json.loads((w/'sources-verified.json').read_text())}
prefix=w/'prefix';tool=next((root/'toolchain').glob('llvm-mingw-*'))/'bin'
env=os.environ.copy();env['PATH']=str(root/'bin')+':'+str(tool)+':'+env['PATH'];env['PYTHONPATH']=str(root/'python-tools');env['PKG_CONFIG_LIBDIR']=str(prefix/'lib/pkgconfig');env['PKG_CONFIG_PATH']='';env['PKG_CONFIG_SYSROOT_DIR']=''
for key,exe in [('CC','clang'),('CXX','clang++'),('AR','ar'),('RANLIB','ranlib'),('NM','nm'),('STRIP','strip')]:env[key]=str(tool/('x86_64-w64-mingw32-'+exe))
env['CFLAGS']='-O2 -I'+str(prefix/'include');env['CPPFLAGS']='-I'+str(prefix/'include');env['LDFLAGS']='-L'+str(prefix/'lib')+' -static'
sys.path.insert(0,str(root/'source/python'))
from build.libs import ffmpeg,libmpg123
assert ffmpeg.version=='9.0.2' and libmpg123.version=='1.33.7'
projects=[
 ('mpg123',[str(Path(sources['mpg123']['source_directory'])/'configure'),'--host=x86_64-w64-mingw32','--build=x86_64-pc-linux-gnu','--prefix='+str(prefix)]+libmpg123.configure_args),
 ('ffmpeg',[str(Path(sources['ffmpeg']['source_directory'])/'configure'),'--cc='+env['CC'],'--cxx='+env['CXX'],'--ar='+env['AR'],'--ranlib='+env['RANLIB'],'--nm='+env['NM'],'--strip='+env['STRIP'],'--enable-cross-compile','--arch=x86_64','--target-os=mingw32','--prefix='+str(prefix),'--pkg-config=/usr/bin/pkg-config','--pkg-config-flags=--static','--extra-cflags='+env['CFLAGS'],'--extra-ldflags='+env['LDFLAGS'],'--disable-x86asm']+ffmpeg.configure_args),
]
receipts=[]
for name,command in projects:
 build=w/(name+'-build');build.mkdir(exist_ok=True);log=w/(name+'-build.log')
 with log.open('w')as out:
  subprocess.run(['nice','-n','10']+command,cwd=build,env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
  subprocess.run(['nice','-n','10','make','-j1'],cwd=build,env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
  subprocess.run(['nice','-n','10','make','install'],cwd=build,env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
 receipts.append({'name':name,'source':sources[name],'configure':command,'compiled_windows_static':True,'executed':False,'mpd_upstream_configure_flags_reused':True,'ffmpeg_external_x86asm':False})
 (w/'codec-build-receipts.json').write_text(json.dumps(receipts,indent=2)+'\n');print(name,'Windows static build/install passed',flush=True)
