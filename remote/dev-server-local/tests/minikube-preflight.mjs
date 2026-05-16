import { spawnSync } from 'node:child_process';

function run(command, args, options = {}) {
  return spawnSync(command, args, {
    encoding: 'utf8',
    ...options,
  });
}

function requireCommand(command, args = ['--version']) {
  const result = run(command, args);
  if (result.status === 0) {
    return;
  }

  const detail = result.stderr || result.stdout || `${command} exited ${result.status}`;
  throw new Error(`Required command "${command}" is not ready.\n${detail.trim()}`);
}

requireCommand('minikube', ['version']);
requireCommand('kubectl', ['version', '--client=true']);
requireCommand('docker', ['--version']);

const dockerInfo = run('docker', ['info']);
if (dockerInfo.status !== 0) {
  const detail = dockerInfo.stderr || dockerInfo.stdout || 'docker info failed';
  throw new Error(
    [
      'Docker is installed, but its daemon is not running.',
      'Start Docker Desktop, then rerun `pnpm run minikube:start` from remote/dev-server-local.',
      detail.trim(),
    ].join('\n'),
  );
}

const context = run('kubectl', ['config', 'current-context']);
if (context.status === 0 && context.stdout.trim() && context.stdout.trim() !== 'minikube') {
  console.warn(
    `kubectl context is ${JSON.stringify(context.stdout.trim())}; minikube:start will switch/use the minikube profile after startup.`,
  );
}

console.log('remote/dev-server-local minikube preflight passed');
