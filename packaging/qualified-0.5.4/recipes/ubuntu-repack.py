"""Metadata-only private DEB revision. No build, installation or audio access.
Usage: python3 repackage-deb.py ORIGINAL.deb OUTPUT.deb
GNU ar and xz are required. Data member is copied byte for byte.
"""
from pathlib import Path
import subprocess, tarfile, io, lzma, hashlib, json, sys, tempfile
src,dst=map(lambda x:Path(x).resolve(),sys.argv[1:])
assert not dst.exists()
sha=lambda b:hashlib.sha256(b).hexdigest()
assert sha(src.read_bytes())=='884d4e7884c79a604d679bc73082d97751664df8ba23f19eba1eea39d0734f5e'
members=subprocess.check_output(['ar','t',str(src)],text=True).splitlines()
assert members==['debian-binary','control.tar.xz','data.tar.xz']
read=lambda n:subprocess.check_output(['ar','p',str(src),n])
old=read('control.tar.xz'); archive=tarfile.open(fileobj=io.BytesIO(old),mode='r:xz')
out=io.BytesIO();before=after=None
with tarfile.open(fileobj=out,mode='w',format=tarfile.GNU_FORMAT) as t:
    for info in archive:
        b=archive.extractfile(info).read() if info.isfile() else None
        if info.name in ('control','./control'):
            before=b.decode(); lines=before.splitlines()
            for i,line in enumerate(lines):
                if line.startswith('Depends: '):
                    deps=line[len('Depends: '):].split(', ');assert deps.count('wireplumber')==1
                    deps.remove('wireplumber');lines[i]='Depends: '+', '.join(deps)
                if line.startswith(('Version: ','Description: ')):lines[i]=line.replace('20261020.1','20261020.2')
            after='\n'.join(lines)+'\n';assert 'wireplumber' not in after
            b=after.encode();info.size=len(b)
        t.addfile(info,io.BytesIO(b) if b is not None else None)
with tempfile.TemporaryDirectory(prefix='sonante-deb-') as tmp:
    root=Path(tmp)
    for n in members:(root/n).write_bytes(lzma.compress(out.getvalue(),format=lzma.FORMAT_XZ,preset=6) if n=='control.tar.xz' else read(n))
    subprocess.run(['ar','rcD',str(dst),*members],cwd=root,check=True)
assert read('data.tar.xz')==subprocess.check_output(['ar','p',str(dst),'data.tar.xz'])
receipt={'original_sha256':sha(src.read_bytes()),'new_sha256':sha(dst.read_bytes()),'data_member_sha256':sha(read('data.tar.xz')),'data_member_byte_identical':True,'only_control_changes':['Version .1 -> .2','Description .1 -> .2','Depends removes wireplumber'],'control_before':before,'control_after':after}
(dst.parent/'metadata-repack-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({k:v for k,v in receipt.items() if not k.startswith('control_')},indent=2))
