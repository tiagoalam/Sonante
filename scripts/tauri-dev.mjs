#!/usr/bin/env node
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const EQ_VERIFIER = path.join(ROOT, 'scripts', 'dev-eq.sh');
const PROFILE_NAME = 'sonante-0.6.4-sqlite';
const NULL_PROFILE_NAME = 'sonante-0.6.4-sqlite-null-test';

function assertPrivateDirectory(directory, uid) {
  const stat = fs.lstatSync(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== uid || (stat.mode & 0o077) !== 0) {
    throw new Error(`Diretório de desenvolvimento ausente ou inseguro: ${directory}`);
  }
}

function prepareProfile(profile, uid, home) {
  const parent = path.dirname(profile);
  for (const directory of [path.join(home, '.local'), parent]) {
    const stat = fs.lstatSync(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== uid) {
      throw new Error(`Diretório pai do perfil ausente ou inseguro: ${directory}`);
    }
  }

  const directories = ['.', 'config', 'data', 'cache', 'state', 'runtime', 'logs']
    .map((entry) => entry === '.' ? profile : path.join(profile, entry));
  for (const directory of directories) {
    try {
      fs.mkdirSync(directory, { mode: 0o700 });
    } catch (error) {
      if (error.code !== 'EEXIST') throw error;
    }
    assertPrivateDirectory(directory, uid);
  }
  return directories;
}

function parseVerifiedEqRuntimes(output) {
  const mpd = output.match(/^MPD EQ privado validado: (.+)$/m)?.[1];
  const camilla = output.match(/^CamillaDSP privado validado: (.+)$/m)?.[1];
  if (!mpd || !camilla || !path.isAbsolute(mpd) || !path.isAbsolute(camilla)) {
    throw new Error('O verificador não confirmou os runtimes privados de EQ/CamillaDSP.');
  }
  return { mpd, camilla };
}

export function prepareTauriDevEnvironment({
  args,
  platform,
  env,
  home,
  uid,
  profileDirectories,
  eqRuntimeOutput,
}) {
  const childEnv = { ...env };
  if (platform !== 'linux' || args[0] !== 'dev') return childEnv;

  const profileName = env.SONANTE_064_TEST_NULL_OUTPUT === '1' ? NULL_PROFILE_NAME : PROFILE_NAME;
  const profile = path.join(home, '.local', 'share', profileName);
  const directories = profileDirectories ?? prepareProfile(profile, uid, home);
  const [root, config, data, cache, state] = directories;
  Object.assign(childEnv, {
    XDG_CONFIG_HOME: config,
    XDG_DATA_HOME: data,
    XDG_CACHE_HOME: cache,
    XDG_STATE_HOME: state,
  });
  // XDG_RUNTIME_DIR belongs to the desktop session and is intentionally kept
  // as inherited so PipeWire clients continue to reach the user's session.
  for (const key of [
    'MPD_HOST', 'MPD_PORT', 'LISTEN_PID', 'LISTEN_FDS', 'LISTEN_FDNAMES',
    'SONANTE_064_RUNTIME_DIR', 'SONANTE_064_PROFILE', 'SONANTE_DEV_MPD_PATH',
    'SONANTE_064_PROFILE_ROOT',
    'SONANTE_EXPERIMENTAL_PROFILE_ROOT', 'SONANTE_EXPERIMENTAL_RUNTIME_ROOT',
    'SONANTE_EQ_MPD_PATH', 'SONANTE_EQ_CAMILLA_PATH', 'SONANTE_EQ_DIAGNOSTICS',
  ]) delete childEnv[key];

  if (env.SONANTE_064_TEST_NULL_OUTPUT !== '1') {
    if (!eqRuntimeOutput) throw new Error('Runtimes EQ/CamillaDSP não foram validados.');
    const { mpd, camilla } = parseVerifiedEqRuntimes(eqRuntimeOutput);
    childEnv.SONANTE_EQ_MPD_PATH = mpd;
    childEnv.SONANTE_EQ_CAMILLA_PATH = camilla;
    childEnv.SONANTE_EQ_DIAGNOSTICS = '1';
  }
  return childEnv;
}

function start() {
  const args = process.argv.slice(2);
  let env;
  try {
    let eqRuntimeOutput;
    if (process.platform === 'linux' && args[0] === 'dev') {
      const profileName = process.env.SONANTE_064_TEST_NULL_OUTPUT === '1' ? NULL_PROFILE_NAME : PROFILE_NAME;
      const profile = path.join(os.homedir(), '.local', 'share', profileName);
      const directories = prepareProfile(profile, process.getuid(), os.homedir());
      const preliminaryEnv = prepareTauriDevEnvironment({
        args, platform: process.platform,
        env: { ...process.env, SONANTE_064_TEST_NULL_OUTPUT: '1' }, home: os.homedir(),
        uid: process.getuid(), profileDirectories: directories,
      });
      if (process.env.SONANTE_064_TEST_NULL_OUTPUT !== '1') {
        delete preliminaryEnv.SONANTE_064_TEST_NULL_OUTPUT;
        const result = spawnSync('bash', [EQ_VERIFIER, '--verify-only'], {
          cwd: ROOT, env: preliminaryEnv, encoding: 'utf8', timeout: 30_000,
        });
        if (result.error || result.status !== 0) {
          throw new Error(`Falha ao validar os runtimes EQ/CamillaDSP: ${(result.stderr || result.stdout || result.error?.message || '').trim()}`);
        }
        eqRuntimeOutput = result.stdout;
      }
    }
    env = prepareTauriDevEnvironment({
      args, platform: process.platform, env: process.env, home: os.homedir(),
      uid: process.getuid?.(), eqRuntimeOutput,
    });
    if (process.platform === 'linux' && args[0] === 'dev') process.umask(0o077);
  } catch (error) {
    console.error(`[Sonante dev] ${error.message}`);
    process.exitCode = 1;
    return;
  }

  const cli = path.join(ROOT, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  const tauriArgs = tauriArgsFor(args, process.platform, ROOT);
  const child = spawn(process.execPath, [cli, ...tauriArgs], { cwd: ROOT, env, stdio: 'inherit' });
  child.once('error', (error) => {
    console.error(`[Sonante dev] Não foi possível iniciar o Tauri: ${error.message}`);
    process.exitCode = 1;
  });
  child.once('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code ?? 1;
    }
  });
}

export function tauriArgsFor(args, platform, root = ROOT) {
  return args[0] === 'dev'
    ? [...args, '--config', path.join(root, 'src-tauri', 'tauri.dev.conf.json')]
    : args;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) start();
