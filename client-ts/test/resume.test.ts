import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { loadSchema } from '../src/taut/schema.ts';
import { GladeClient } from '../src/client.ts';

const schema = loadSchema(JSON.parse(readFileSync(new URL('../../../taut/corpus/glade.ir.json', import.meta.url), 'utf8')));
test('retained state resumes only missing tail; unconfirmed local edits remain resendable', { timeout: 15000 }, async () => {
  const directory = mkdtempSync(`${tmpdir()}/glade-resume-`);
  const child = spawn(fileURLToPath(new URL('../../node/target/debug/glade-node', import.meta.url)), ['0', directory], { stdio: ['ignore', 'pipe', 'pipe'] });
  const clients: GladeClient[] = [];
  try {
    const port = await new Promise<number>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('node startup timeout')), 8000);
      child.once('error', reject);
      child.stdout.on('data', data => {
        const match = /listening (\d+)/.exec(String(data));
        if (match) { clearTimeout(timer); resolve(Number(match[1])); }
      });
      child.once('exit', code => { clearTimeout(timer); reject(new Error(`node exited ${code}`)); });
    });
    const url = `ws://127.0.0.1:${port}`;
    const writer = new GladeClient(schema, 'writer'); clients.push(writer);
    await writer.connect(url);
    await writer.hello();
    await writer.subscribe('sh', 'resume');
    for (let n = 0; n < 12; n++) { assert.equal((await writer.appendOutcome('sh', 'resume', 'log', new TextEncoder().encode(`${n}`))).ok, true); }
    const baseline = writer.session.dump().filter(o => o.glade_id === 'resume');
    await writer.appendOutcome('sh', 'resume', 'log', new TextEncoder().encode('tail'));
    const reader = new GladeClient(schema, 'reader'); clients.push(reader);
    reader.session.applyRemote(baseline);
    const pending = reader.session.append('sh', 'resume', 'log', new TextEncoder().encode('unsent'));
    let received = 0;
    reader.onOps = ops => { received += ops.length; reader.session.applyRemote(ops); };
    await reader.connect(url);
    await reader.hello(undefined, { resume: true });
    const result = await reader.subscribeOutcome('sh', 'resume');
    assert.equal(result.ok, true);
    assert.equal(received, 1);
    assert.equal((await reader.sendOpsOutcome([pending]))[0]?.ok, true);
    assert.equal(reader.session.dump().filter(o => o.glade_id === 'resume').length, 14);
  } finally {
    for (const client of clients) { client.close(); }
    child.kill();
    await new Promise<void>(resolve => { if (child.exitCode !== null) { resolve(); } else { child.once('exit', () => resolve()); } });
    rmSync(directory, { recursive: true, force: true });
  }
});
