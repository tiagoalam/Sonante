from pathlib import Path
import subprocess,json,os,sys
w=Path(__file__).resolve().parent;root=w.parent
sources={r['name']:r for r in json.loads((w/'sources-verified.json').read_text())}
prefix=w/'prefix';prefix.mkdir(exist_ok=True)
tool=next((root/'toolchain').glob('llvm-mingw-*'))/'bin'
config=w/'toolchain.cmake'
config.write_text('\n'.join([
 'set(CMAKE_SYSTEM_NAME Windows)','set(CMAKE_SYSTEM_PROCESSOR AMD64)',
 f'set(CMAKE_C_COMPILER "{tool}/x86_64-w64-mingw32-clang")',
 f'set(CMAKE_CXX_COMPILER "{tool}/x86_64-w64-mingw32-clang++")',
 f'set(CMAKE_RC_COMPILER "{tool}/x86_64-w64-mingw32-windres")',
 f'set(CMAKE_AR "{tool}/x86_64-w64-mingw32-ar")',
 f'set(CMAKE_RANLIB "{tool}/x86_64-w64-mingw32-ranlib")',
 f'set(CMAKE_FIND_ROOT_PATH "{prefix}" "{tool.parent}/x86_64-w64-mingw32")',
 'set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)','set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)',
 'set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)','set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)',
])+'\n')
env=os.environ.copy();env['PATH']=str(root/'bin')+':'+str(tool)+':'+env['PATH'];env['PYTHONPATH']=str(root/'python-tools');env['PKG_CONFIG_LIBDIR']=str(prefix/'lib/pkgconfig')
projects=[
 ('zlib',['-DZLIB_BUILD_TESTING=OFF','-DZLIB_BUILD_SHARED=OFF','-DZLIB_BUILD_STATIC=ON']),
 ('libsamplerate',['-DBUILD_SHARED_LIBS=OFF','-DBUILD_TESTING=OFF','-DLIBSAMPLERATE_EXAMPLES=OFF','-DLIBSAMPLERATE_INSTALL=ON','-DINSTALL_DOCS=OFF','-DINSTALL_CMAKE_PACKAGE_MODULE=OFF','-DCMAKE_POLICY_VERSION_MINIMUM=3.5']),
 ('curl',['-DBUILD_SHARED_LIBS=OFF','-DBUILD_STATIC_LIBS=ON','-DBUILD_CURL_EXE=OFF','-DBUILD_TESTING=OFF','-DBUILD_LIBCURL_DOCS=OFF','-DBUILD_MISC_DOCS=OFF','-DCURL_USE_SCHANNEL=ON','-DCURL_USE_OPENSSL=OFF','-DCURL_USE_LIBPSL=OFF','-DUSE_LIBIDN2=OFF','-DUSE_WIN32_IDN=ON','-DUSE_NGHTTP2=OFF','-DCURL_USE_LIBSSH2=OFF','-DCURL_USE_LIBSSH=OFF','-DCURL_USE_GSSAPI=OFF','-DCURL_BROTLI=OFF','-DCURL_ZSTD=OFF','-DCURL_ZLIB=ON','-DHTTP_ONLY=ON']),
]
receipts=[]
for name,flags in projects:
 if len(sys.argv)>1 and name not in sys.argv[1:]:continue
 source=Path(sources[name]['source_directory']);build=w/(name+'-build');log=w/(name+'-build.log')
 command=['cmake','-S',str(source),'-B',str(build),'-GNinja',f'-DCMAKE_MAKE_PROGRAM={root}/bin/ninja',f'-DCMAKE_TOOLCHAIN_FILE={config}',f'-DCMAKE_INSTALL_PREFIX={prefix}','-DCMAKE_BUILD_TYPE=Release']+flags
 with log.open('w')as out:
  subprocess.run(['nice','-n','10']+command,env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
  subprocess.run(['nice','-n','10','cmake','--build',str(build),'--parallel','1'],env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
  subprocess.run(['nice','-n','10','cmake','--install',str(build)],env=env,stdout=out,stderr=subprocess.STDOUT,check=True)
 receipts.append({'name':name,'source':sources[name],'configure':command,'compiled_windows_static':True,'executed':False})
 (w/'cmake-build-receipts.json').write_text(json.dumps(receipts,indent=2)+'\n');print(name,'Windows static build/install passed',flush=True)
