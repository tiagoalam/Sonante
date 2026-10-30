"""Packaging SHA-256 compatibility without Python3.11 hashlib.file_digest; no engines."""
from contextlib import contextmanager
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT=Path.cwd()
def module(name):
 spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/name)
 result=importlib.util.module_from_spec(spec);spec.loader.exec_module(result);return result

@contextmanager
def old_hashlib_api():
 old=getattr(hashlib,'file_digest',None)
 if old is not None:delattr(hashlib,'file_digest')
 try:yield
 finally:
  if old is not None:hashlib.file_digest=old

def digest(data):return hashlib.sha256(data).hexdigest()

class PackagingPythonCompatibility(unittest.TestCase):
 def test_existing_file_hashers_work_without_new_stdlib_api(self):
  with tempfile.TemporaryDirectory(prefix='sn-pyhash-') as directory:
   path=Path(directory)/'input'
   for script in ['qualified-package.py','stage-private-runtimes.py']:
    tool=module(script)
    for data in [b'',b'abc',bytes(range(256))*8201]:
     with self.subTest(script=script,bytes=len(data)):
      path.write_bytes(data)
      with old_hashlib_api():self.assertEqual(tool.digest(path),digest(data))

 def test_qualification_checks_streamed_tar_hash_and_refuses_tampering(self):
  tool=module('qualified-package.py');data=b'opaque runtime archive fixture'*80000
  with tempfile.TemporaryDirectory(prefix='sn-pytar-') as directory:
   path=Path(directory)/'inputs.tar'
   for tamper in [False,True]:
    archive=io.BytesIO()
    with tarfile.open(fileobj=archive,mode='w') as tar:
     item=tarfile.TarInfo('inputs/runtime.tar');item.size=len(data)
     tar.addfile(item,io.BytesIO(data if not tamper else b'x'*len(data)))
    path.write_bytes(archive.getvalue())
    profile={'external_inputs':{'sha256':digest(path.read_bytes()),'prefix':'inputs'},'selected_inputs':{'runtime.tar':digest(data)}}
    @contextmanager
    def decompressed(_):yield io.BytesIO(archive.getvalue())
    with self.subTest(tamper=tamper),mock.patch.object(tool,'archive_stream',decompressed),old_hashlib_api():
     if tamper:
      with self.assertRaisesRegex(ValueError,'Selected build input changed'):tool.inspect_inputs(path,profile)
     else:tool.inspect_inputs(path,profile)

 def test_runtime_stage_file_check_keeps_identity_guard(self):
  tool=module('stage-private-runtimes.py')
  with tempfile.TemporaryDirectory(prefix='sn-pystage-') as directory:
   path=Path(directory)/'runtime';path.write_bytes(b'nonexecuted runtime fixture')
   with old_hashlib_api():
    self.assertEqual(tool.checked({'source':str(path),'sha256':digest(path.read_bytes())}),path)
    with self.assertRaisesRegex(ValueError,'Input identity mismatch'):
     tool.checked({'source':str(path),'sha256':'0'*64})

 def test_arch_check_only_preserves_all_input_hash_refusals(self):
  tool=module('prepare-private-arch.py')
  for changed,message in [(None,None),('source','Snapshot changed'),('sidecar','source sidecar'),('application','Application does not match')]:
   with self.subTest(changed=changed),tempfile.TemporaryDirectory(prefix='sn-pyarch-') as directory:
    root=Path(directory);source=root/'source';source.mkdir();file=source/'file.rs';file.write_bytes(b'fixture source');file.chmod(0o644)
    manifest=root/'sources.json';manifest.write_text(json.dumps({'file.rs':{'mode':0o644,'sha256':digest(file.read_bytes())}}))
    sidecar=root/'runtime-sources.tar';sidecar.write_bytes(b'opaque source sidecar fixture')
    stage=root/'stage';inventory=stage/'usr/share/doc/sonante/runtime-inventory.json';inventory.parent.mkdir(parents=True)
    inventory.write_text(json.dumps({'source_sidecar':{'filename':sidecar.name,'sha256':digest(sidecar.read_bytes())},'arch_packages':[]}))
    app=root/'application';app.write_bytes(b'nonexecuted binary fixture')
    receipt=root/'app.json';receipt.write_text(json.dumps({'sha256':digest(app.read_bytes()),'source_manifest_sha256':digest(manifest.read_bytes()),'custom_protocol':True}))
    if changed=='source':file.write_bytes(b'changed source')
    if changed=='sidecar':sidecar.write_bytes(b'changed sidecar')
    if changed=='application':app.write_bytes(b'changed application')
    output=root/'not-created'
    with mock.patch.object(tool,'SOURCE',source),mock.patch.object(tool.verifier,'verify',return_value={}) as stage_check,mock.patch.object(tool.subprocess,'check_output',return_value='Name: GLIBC_2.35'),old_hashlib_api():
     if changed:
      with self.assertRaisesRegex(ValueError,message):tool.prepare(stage,manifest,sidecar,app,receipt,output,check_only=True)
     else:tool.prepare(stage,manifest,sidecar,app,receipt,output,check_only=True)
     stage_check.assert_called_once_with(stage)
    self.assertFalse(output.exists())

 def test_arch_source_archive_hash_and_contents_without_new_api(self):
  tool=module('prepare-private-arch.py')
  with tempfile.TemporaryDirectory(prefix='sn-pyarchive-') as directory:
   root=Path(directory);file=root/'file.rs';file.write_bytes(b'fixture source');file.chmod(0o644);archive=root/'source.tar'
   with old_hashlib_api():actual=tool.archive(archive,root,['file.rs'],'source')
   self.assertEqual(actual,digest(archive.read_bytes()))
   with tarfile.open(archive) as tar:
    member=tar.getmember('source/file.rs');self.assertEqual(tar.extractfile(member).read(),b'fixture source')
    self.assertEqual((member.uid,member.gid,member.mode),(0,0,0o644))

if __name__=='__main__':unittest.main(verbosity=2)
