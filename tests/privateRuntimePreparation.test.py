"""Opt-in preparation integration tests against an explicit private task fixture."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parent.parent
TASK=Path(os.environ['SONANTE_RUNTIME_TEST_TASK']) if os.environ.get('SONANTE_RUNTIME_TEST_TASK') else None

def module(name):
 spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/name)
 result=importlib.util.module_from_spec(spec);spec.loader.exec_module(result);return result

@unittest.skipUnless(TASK,'explicit private preparation fixture required')
class RuntimePreparation(unittest.TestCase):
 def setUp(self):
  self.out=TASK/'outputs/sonante-runtime-remap';self.stage=TASK/'work/ubuntu-runtime-stage'
  self.record=self.out/'validated-runtime-build-inputs.json';self.sidecar=self.out/'runtime-source-materials-v2.tar.zst'
 def test_exact_stage_inventory_is_verified(self):
  receipt=module('verify-private-runtime-stage.py').verify(self.stage)
  self.assertFalse(any('/runtime-sources/' in p for p in receipt))
  self.assertEqual(sum(p.startswith('usr/lib/sonante/runtimes/v1/') for p in receipt),115)
 def test_nonadmitted_and_changed_resource_are_rejected_before_output(self):
  original=json.loads(self.record.read_text());stage=module('stage-private-runtimes.py')
  for kind in ['nonadmitted','resource']:
   data=copy.deepcopy(original)
   if kind=='nonadmitted':data['admitted']=False
   else:data['files']['mpd-direct']['sha256']='0'*64
   with tempfile.TemporaryDirectory() as directory:
    base=Path(directory);record=base/'record.json';record.write_text(json.dumps(data));dest=base/'not-created'
    with self.assertRaises(ValueError):stage.prepare(record,self.sidecar,dest)
    self.assertFalse(dest.exists())
 def test_wrong_source_sidecar_is_rejected_before_output(self):
  with tempfile.TemporaryDirectory() as directory:
   base=Path(directory);sidecar=base/self.sidecar.name;sidecar.write_bytes(b'changed');dest=base/'not-created'
   with self.assertRaisesRegex(ValueError,'sidecar mismatch'):module('stage-private-runtimes.py').prepare(self.record,sidecar,dest)
   self.assertFalse(dest.exists())
 def test_audit_tree_cannot_become_package_input(self):
  with tempfile.TemporaryDirectory() as directory:
   unused=Path(directory)/'not-created'
   with self.assertRaisesRegex(ValueError,'blocked'):
    module('prepare-private-arch.py').prepare(TASK/'work/private-runtime-audit-stage',unused,unused,unused,unused,unused)
   self.assertFalse(unused.exists())
 def preflight(self,data):
  with tempfile.TemporaryDirectory() as directory:
   p=Path(directory)/'abi.json';p.write_text(json.dumps(data))
   result=subprocess.run(['python3','-I',str(ROOT/'scripts/check-private-runtime-host.py'),str(p)],capture_output=True,text=True)
   return result.returncode,json.loads(result.stdout)
 def test_current_host_preflight_passes_without_destination_claim(self):
  code,result=self.preflight(json.loads((self.stage/'usr/share/doc/sonante/runtime-inventory.json').read_text()))
  self.assertEqual(code,0);self.assertTrue(result['passed']);self.assertEqual(result['required_glibc'],'2.35')
 def test_older_glibc_missing_soname_cpp_and_webkit_are_refused(self):
  original=json.loads((self.stage/'usr/share/doc/sonante/runtime-inventory.json').read_text())
  for failure in ['glibc','soname','cpp','webkit']:
   data=copy.deepcopy(original)
   if failure=='glibc':data['abi']['glibc_ceiling']='999.0'
   if failure=='soname':data['system_libraries'].append('libsonante-missing-fixture.so.999')
   if failure=='cpp':data['abi']['glibcxx_ceiling']='999.0'
   if failure=='webkit':data['abi']['webkit_minimum']='999.0'
   with self.subTest(failure=failure):
    code,result=self.preflight(data);self.assertEqual(code,1);self.assertFalse(result['passed'])

if __name__=='__main__':unittest.main(verbosity=2)
