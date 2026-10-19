import { build } from 'esbuild';
import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const outdir = new URL('../../target/browser/', import.meta.url);
await mkdir(outdir, { recursive: true });
await build({
  absWorkingDir: fileURLToPath(new URL('.', import.meta.url)),
  entryPoints: { relay: 'relay.mjs' },
  outdir: fileURLToPath(outdir),
  bundle: true,
  platform: 'browser',
  format: 'esm',
  target: 'es2022',
  sourcemap: true,
  logLevel: 'info',
});
await copyFile(new URL('browser/check.mjs', import.meta.url), new URL('check.js', outdir));
await copyFile(new URL('browser/index.html', import.meta.url), new URL('index.html', outdir));
