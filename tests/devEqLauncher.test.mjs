import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, chmodSync, rmSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const repo = path.resolve(import.meta.dirname, '..');
const launcher = path.join(repo, 'scripts/dev-eq.sh');
const mpdSource = readFileSync(path.join(repo, 'src-tauri/src/mpd_engine.rs'), 'utf8');
const camillaSource = readFileSync(path.join(repo, 'src-tauri/src/shared_dsp/binary.rs'), 'utf8');

function pinned(source, expression) {
  return source.match(expression)?.[1];
}

test('EQ launcher pins match the Rust runtime validators', () => {
  const script = readFileSync(launcher, 'utf8');
  assert.equal(pinned(script, /MPD_EQ_SHA256="([a-f0-9]{64})"/),
    pinned(mpdSource, /DEV_EQ_SHA256: &str = "([a-f0-9]{64})"/));
  assert.notEqual(pinned(script, /MPD_EQ_SHA256="([a-f0-9]{64})"/),
    pinned(mpdSource, /INSTALLED_EQ_SHA256: &str = "([a-f0-9]{64})"/));
  assert.equal(pinned(script, /CAMILLA_SHA256="([a-f0-9]{64})"/),
    pinned(camillaSource, /CAMILLA_SHA256: &str = "([a-f0-9]{64})"/));
});
test('EQ launcher defaults to the isolated 0.6.4 private runtime directory', () => {
  const script = readFileSync(launcher, 'utf8');
  assert.match(script, /\.local\/share\/sonante-0\.6\.4-sqlite\/runtime\/eq-dev/);
  assert.match(script, /set -- "\$PRIVATE_EQ_RUNTIME\/mpd-eq" "\$PRIVATE_EQ_RUNTIME\/camilladsp"/);
  const bootstrap = readFileSync(path.join(repo, 'src-tauri/src/lib.rs'), 'utf8');
  const ambientClear = bootstrap.match(/for key in \[([\s\S]*?)\] \{\s*std::env::remove_var\(key\)/)?.[1];
  assert.match(ambientClear, /"SONANTE_DEV_MPD_PATH"/);
  assert.doesNotMatch(ambientClear, /SONANTE_EQ_MPD_PATH/);
  assert.match(bootstrap, /if null_output_test\s*\{[\s\S]*?SONANTE_EQ_MPD_PATH[\s\S]*?SONANTE_EQ_CAMILLA_PATH/);
});
test('EQ launcher refuses null mode before checking or launching runtimes', () => {
  const result = spawnSync(launcher, ['/missing/mpd-eq', '/missing/camilladsp'], {
    encoding: 'utf8',
    env: { ...process.env, SONANTE_064_TEST_NULL_OUTPUT: '1' },
  });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /força o modo null/);
  assert.doesNotMatch(result.stdout, /MPD EQ privado validado/);
});

test('verify-only rejects absent and altered runtime files without starting Tauri', () => {
  const root = mkdtempSync(path.join(tmpdir(), 'sonante-eq-launcher-'));
  try {
    const mpd = path.join(root, 'mpd-eq');
    const camilla = path.join(root, 'camilladsp');
    writeFileSync(mpd, '#!/bin/sh\nexit 0\n');
    writeFileSync(camilla, '#!/bin/sh\nexit 0\n');
    chmodSync(mpd, 0o700);
    chmodSync(camilla, 0o700);

    const missing = spawnSync(launcher, ['--verify-only', path.join(root, 'missing'), camilla], { encoding: 'utf8' });
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /MPD EQ inválido/);

    const altered = spawnSync(launcher, ['--verify-only', mpd, camilla], { encoding: 'utf8' });
    assert.notEqual(altered.status, 0);
    assert.match(altered.stderr, /SHA-256 não corresponde/);
    assert.doesNotMatch(altered.stdout, /MPD EQ privado validado/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
