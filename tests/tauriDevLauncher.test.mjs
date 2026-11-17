import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { prepareTauriDevEnvironment } from '../scripts/tauri-dev.mjs';

const runtimeOutput = (profile) => [
  `MPD EQ privado validado: ${profile}/runtime/eq-dev/mpd-eq`,
  `CamillaDSP privado validado: ${profile}/runtime/eq-dev/camilladsp`,
].join('\n');

function fixtureProfile() {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'sonante-dev-launcher-'));
  const profile = path.join(home, '.local', 'share', 'sonante-0.6.4-sqlite');
  fs.mkdirSync(profile, { recursive: true, mode: 0o700 });
  for (const name of ['config', 'data', 'cache', 'state', 'runtime', 'logs']) {
    fs.mkdirSync(path.join(profile, name), { mode: 0o700 });
  }
  for (const directory of [path.join(home, '.local'), path.join(home, '.local', 'share'), profile,
    ...['config', 'data', 'cache', 'state', 'runtime', 'logs'].map((name) => path.join(profile, name))]) {
    fs.chmodSync(directory, 0o700);
  }
  return { home, profile };
}

test('Linux dev prepares isolated XDG and validated EQ runtimes without changing the desktop runtime', () => {
  const { home, profile } = fixtureProfile();
  try {
    const env = prepareTauriDevEnvironment({
      args: ['dev'], platform: 'linux', home, uid: process.getuid(),
      env: {
        HOME: home, XDG_RUNTIME_DIR: '/run/user/1000', XDG_CONFIG_HOME: '/stable/config',
        MPD_HOST: 'tcp:example', MPD_PORT: '6600', LISTEN_PID: '10',
        SONANTE_DEV_MPD_PATH: '/system/mpd', SONANTE_EQ_MPD_PATH: '/ambient/mpd-eq',
        SONANTE_EXPERIMENTAL_PROFILE_ROOT: '/other/profile', SONANTE_EXPERIMENTAL_RUNTIME_ROOT: '/other/runtime',
      },
      eqRuntimeOutput: runtimeOutput(profile),
    });
    assert.equal(env.XDG_CONFIG_HOME, path.join(profile, 'config'));
    assert.equal(env.XDG_DATA_HOME, path.join(profile, 'data'));
    assert.equal(env.XDG_CACHE_HOME, path.join(profile, 'cache'));
    assert.equal(env.XDG_STATE_HOME, path.join(profile, 'state'));
    assert.equal(env.XDG_RUNTIME_DIR, '/run/user/1000');
    assert.equal(env.SONANTE_EQ_MPD_PATH, `${profile}/runtime/eq-dev/mpd-eq`);
    assert.equal(env.SONANTE_EQ_CAMILLA_PATH, `${profile}/runtime/eq-dev/camilladsp`);
    assert.equal(env.SONANTE_EQ_DIAGNOSTICS, '1');
    for (const key of ['MPD_HOST', 'MPD_PORT', 'LISTEN_PID', 'SONANTE_DEV_MPD_PATH',
      'SONANTE_EXPERIMENTAL_PROFILE_ROOT', 'SONANTE_EXPERIMENTAL_RUNTIME_ROOT']) assert.equal(env[key], undefined);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test('null-output dev preserves its explicit guard and does not inject EQ runtimes', () => {
  const { home } = fixtureProfile();
  try {
    const env = prepareTauriDevEnvironment({
      args: ['dev'], platform: 'linux', home, uid: process.getuid(),
      env: { HOME: home, SONANTE_064_TEST_NULL_OUTPUT: '1', SONANTE_EQ_MPD_PATH: '/ambient/mpd' },
    });
    assert.equal(env.SONANTE_064_TEST_NULL_OUTPUT, '1');
    assert.equal(env.XDG_CONFIG_HOME, path.join(home, '.local', 'share', 'sonante-0.6.4-sqlite-null-test', 'config'));
    assert.equal(env.SONANTE_EQ_MPD_PATH, undefined);
    assert.equal(env.SONANTE_EQ_CAMILLA_PATH, undefined);
    assert.equal(env.SONANTE_EQ_DIAGNOSTICS, undefined);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test('Windows and non-dev Tauri commands pass through without Linux runtime setup', () => {
  const env = { XDG_RUNTIME_DIR: '/desktop', SONANTE_EQ_MPD_PATH: '/existing' };
  assert.deepEqual(prepareTauriDevEnvironment({ args: ['dev'], platform: 'win32', env }), env);
  assert.deepEqual(prepareTauriDevEnvironment({ args: ['build'], platform: 'linux', env }), env);
});

test('dev refuses missing EQ runtime verification rather than silently disabling the feature', () => {
  const { home } = fixtureProfile();
  try {
    assert.throws(() => prepareTauriDevEnvironment({
      args: ['dev'], platform: 'linux', home, uid: process.getuid(), env: { HOME: home },
    }), /não foram validados/);
  } finally {
    fs.rmSync(home, { recursive: true, force: true });
  }
});

test('runtime output must identify absolute MPD EQ and CamillaDSP executables', () => {
  assert.throws(() => prepareTauriDevEnvironment({
    args: ['dev'], platform: 'linux', home: '/home/test', uid: process.getuid(),
    profileDirectories: ['/p', '/p/config', '/p/data', '/p/cache', '/p/state'],
    env: {}, eqRuntimeOutput: 'MPD EQ privado validado: relative',
  }), /não confirmou/);
});
